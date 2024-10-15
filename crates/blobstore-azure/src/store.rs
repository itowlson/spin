use std::sync::Arc;
use tokio::sync::Mutex;

use anyhow::Result;
// use azure_data_cosmos::{
//     prelude::{AuthorizationToken, CollectionClient, CosmosClient, Query},
//     CosmosEntity,
// };
use azure_storage_blobs::prelude::{BlobServiceClient, ContainerClient};
// use futures::StreamExt;
// use serde::{Deserialize, Serialize};
use spin_core::async_trait;
use spin_factor_blobstore::{Error, Container, ContainerManager};

pub struct BlobStoreAzureBlob {
    client: BlobServiceClient,
    // client: CollectionClient,
}

/// Azure Cosmos Key / Value runtime config literal options for authentication
#[derive(Clone, Debug)]
pub struct BlobStoreAzureRuntimeConfigOptions {
    account: String,
    key: String,
}

impl BlobStoreAzureRuntimeConfigOptions {
    pub fn new(account: String, key: String) -> Self {
        Self { account, key }
    }
}

/// Azure Cosmos Key / Value enumeration for the possible authentication options
#[derive(Clone, Debug)]
pub enum BlobStoreAzureAuthOptions {
    /// Runtime Config values indicates the account and key have been specified directly
    RuntimeConfigValues(BlobStoreAzureRuntimeConfigOptions),
    /// Environmental indicates that the environment variables of the process should be used to
    /// create the StorageCredentials for the storage client. For now this uses old school credentials:
    /// 
    /// STORAGE_ACCOUNT
    /// STORAGE_ACCESS_KEY
    /// 
    /// TODO: Thorsten pls make this proper with *hand waving* managed identity and stuff!
    Environmental,
}

impl BlobStoreAzureBlob {
    pub fn new(
        // account: String,
        // container: String,
        auth_options: BlobStoreAzureAuthOptions,
    ) -> Result<Self> {
        let (account, credentials) = match auth_options {
            BlobStoreAzureAuthOptions::RuntimeConfigValues(config) => {
                (config.account.clone(), azure_storage::StorageCredentials::access_key(&config.account, config.key.clone()))
            },
            BlobStoreAzureAuthOptions::Environmental => {
                let account = std::env::var("STORAGE_ACCOUNT").expect("missing STORAGE_ACCOUNT");
                let access_key = std::env::var("STORAGE_ACCESS_KEY").expect("missing STORAGE_ACCOUNT_KEY");
                (account.clone(), azure_storage::StorageCredentials::access_key(account, access_key))
            },
        };

        let client = azure_storage_blobs::prelude::ClientBuilder::new(account, credentials).blob_service_client();
        Ok(Self { client })
    }
}

#[async_trait]
impl ContainerManager for BlobStoreAzureBlob {
    async fn get(&self, name: &str) -> Result<Arc<dyn Container>, Error> {
        Ok(Arc::new(AzureBlobContainer {
            _name: name.to_owned(),
            client: self.client.container_client(name),
        }))
    }

    fn is_defined(&self, _store_name: &str) -> bool {
        true
    }

    fn summary(&self, _store_name: &str) -> Option<String> {
        Some(format!("Azure blob storage account {}", self.client.account()))
    }
}

struct AzureBlobContainer {
    _name: String,
    client: ContainerClient,
}

#[async_trait]
impl Container for AzureBlobContainer {
    async fn exists(&self) -> anyhow::Result<bool> {
        Ok(self.client.exists().await?)
    }

    async fn name(&self) -> String {
        self.client.container_name().to_owned()
    }

    async fn clear(&self) -> anyhow::Result<()> {
        anyhow::bail!("Azure blob storage does not support clearing containers")
    }

    async fn delete_object(&self, name: &str) -> anyhow::Result<()> {
        self.client.blob_client(name).delete().await?;
        Ok(())
    }

    async fn delete_objects(&self, names: &[String]) -> anyhow::Result<()> {
        // TODO: are atomic semantics required? or efficiency guarantees?
        let futures = names.iter().map(|name| self.delete_object(name));
        futures::future::try_join_all(futures).await?;
        Ok(())
    }

    async fn has_object(&self, name: &str) -> anyhow::Result<bool> {
        Ok(self.client.blob_client(name).exists().await?)
    }

    async fn object_info(&self, name: &str) -> anyhow::Result<spin_factor_blobstore::ObjectMetadata> {
        let response = self.client.blob_client(name).get_properties().await?;
        Ok(spin_factor_blobstore::ObjectMetadata {
            name: name.to_string(),
            container: self.client.container_name().to_string(),
            created_at: response.blob.properties.creation_time.unix_timestamp().try_into().unwrap(),
            size: response.blob.properties.content_length,
        })
    }

    async fn get_data(&self, name: &str, start: u64, end: u64) -> anyhow::Result<Box<dyn spin_factor_blobstore::IncomingData>> {
        // We can't use a Rust range because the Azure type does not accept inclusive ranges,
        // and we don't want to add 1 to `end` if it's already at MAX!
        let range = if end == u64::MAX {
            azure_core::request_options::Range::RangeFrom(start..)
        } else {
            azure_core::request_options::Range::Range(start..(end + 1))
        };
        let stm = self.client.blob_client(name).get().range(range).into_stream();
        Ok(Box::new(AzureBlobIncomingData(Mutex::new(stm))))
    }

    async fn list_objects(&self) -> anyhow::Result<Box<dyn spin_factor_blobstore::ObjectNames>> {
        let stm = self.client.list_blobs().into_stream();
        Ok(Box::new(AzureBlobBlobsList::new(stm)))
    }
}

struct AzureBlobIncomingData(
    // The Mutex is used to make it Send
    Mutex<
        azure_core::Pageable<
            azure_storage_blobs::blob::operations::GetBlobResponse,
            azure_core::error::Error
        >
    >
);


// struct AzureBlobBlobsList(
//     // The Mutex is used to make it Send
//     Mutex<
//         azure_core::Pageable<
//             azure_storage_blobs::container::operations::ListBlobsResponse,
//             azure_core::error::Error
//         >
//     >
// );

struct AzureBlobBlobsList {
    // The Mutex is used to make it Send
    stm: Mutex<
        azure_core::Pageable<
            azure_storage_blobs::container::operations::ListBlobsResponse,
            azure_core::error::Error
        >
    >,
    read_but_not_yet_returned: Vec<String>,
    end_stm_after_read_but_not_yet_returned: bool,
}

impl AzureBlobBlobsList {
    fn new(stm: azure_core::Pageable<
        azure_storage_blobs::container::operations::ListBlobsResponse,
        azure_core::error::Error
    >) -> Self {
        Self {
            stm: Mutex::new(stm),
            read_but_not_yet_returned: Default::default(),
            end_stm_after_read_but_not_yet_returned: false,
        }
    }

    async fn read_impl(&mut self, len: u64) -> anyhow::Result<(Vec<String>,bool)> {
        use futures::StreamExt;

        let len: usize = len.try_into().unwrap();

        // If we have names outstanding, send that first.  (We are allowed to send less than len,
        // and so sending all pending stuff before paging, rather than trying to manage a mix of
        // pending stuff with newly retrieved chunks, simplifies the code.)
        if !self.read_but_not_yet_returned.is_empty() {
            if self.read_but_not_yet_returned.len() <= len {
                // We are allowed to send all pending names
                let to_return = self.read_but_not_yet_returned.drain(..).collect();
                return Ok((to_return, self.end_stm_after_read_but_not_yet_returned));
            } else {
                // Send as much as we can. The rest remains in the pending buffer to send,
                // so this does not represent end of stream.
                let to_return = self.read_but_not_yet_returned.drain(0..len).collect();
                return Ok((to_return, false));
            }
        }

        // Get one chunk and send as much as we can of it. Aagin, we don't need to try to
        // pack the full length here - we can send chunk by chunk.

        let Some(chunk) = self.stm.get_mut().next().await else {
            return Ok((vec![], false));
        };
        let chunk = chunk.unwrap();

        // TODO: do we need to prefix these with a prefix from somewhere or do they include it?
        let mut names: Vec<_> = chunk.blobs.blobs().map(|blob| blob.name.clone()).collect();
        let at_end = chunk.next_marker.is_none();

        if names.len() <= len {
            // We can send them all!
            return Ok((names, at_end));
        } else {
            // We have more names than we can send in this response. Send what we can and
            // stash the rest.
            let to_return: Vec<_> = names.drain(0..len).collect();
            self.read_but_not_yet_returned = names;
            self.end_stm_after_read_but_not_yet_returned = at_end;
            return Ok((to_return, false));
        }
    }
}

#[async_trait]
impl spin_factor_blobstore::IncomingData for AzureBlobIncomingData {
    async fn consume_sync(&mut self) -> anyhow::Result<Vec<u8>> {
        use futures::StreamExt;
        let mut data = vec![];
        let pageable = self.0.get_mut();

        loop {
            let Some(chunk) = pageable.next().await else {
                break;
            };
            let chunk = chunk.unwrap();
            let by = chunk.data.collect().await.unwrap();
            data.extend(by.to_vec());
        }

        Ok(data)
    }
}

#[async_trait]
impl spin_factor_blobstore::ObjectNames for AzureBlobBlobsList {
    async fn read(&mut self, len: u64) -> anyhow::Result<(Vec<String>, bool)> {
        self.read_impl(len).await  // Separate function because rust-analyser gives better intellisense when async_trait isn't in the picture!
    }

    async fn skip(&mut self, num: u64) -> anyhow::Result<(u64, bool)> {
        // TODO: there is a question (raised as an issue on the repo) about the required behaviour
        // here. For now I assume that skipping fewer than `num` is allowed as long as we are
        // honest about it. Because it is easier that is why.
        let (skipped, at_end) = self.read_impl(num).await?;
        Ok((skipped.len().try_into().unwrap(), at_end))
    }
}
