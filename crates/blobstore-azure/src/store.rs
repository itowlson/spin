use std::sync::Arc;

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

    // async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, Error> {
    //     let pair = self.get_pair(key).await?;
    //     Ok(pair.map(|p| p.value))
    // }

    // async fn set(&self, key: &str, value: &[u8]) -> Result<(), Error> {
    //     let pair = Pair {
    //         id: key.to_string(),
    //         value: value.to_vec(),
    //     };
    //     self.client
    //         .create_document(pair)
    //         .is_upsert(true)
    //         .await
    //         .map_err(log_error)?;
    //     Ok(())
    // }

    // async fn delete(&self, key: &str) -> Result<(), Error> {
    //     if self.exists(key).await? {
    //         let document_client = self.client.document_client(key, &key).map_err(log_error)?;
    //         document_client.delete_document().await.map_err(log_error)?;
    //     }
    //     Ok(())
    // }

    // async fn exists(&self, key: &str) -> Result<bool, Error> {
    //     Ok(self.get_pair(key).await?.is_some())
    // }

    // async fn get_keys(&self) -> Result<Vec<String>, Error> {
    //     self.get_keys().await
    // }
}
