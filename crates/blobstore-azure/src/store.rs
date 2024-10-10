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
