use std::time::Duration;

use redis::{
    Client,
    aio::{ConnectionManager, ConnectionManagerConfig},
};

use crate::Result;

#[derive(Debug, Clone)]
pub enum RedisClient {
    Url(String),
    Manager(ConnectionManager),
    Client(Client),
}

impl RedisClient {
    pub async fn build_connection_manager_from_url(url: &str) -> Result<ConnectionManager> {
        let client = Client::open(url)?;
        Self::build_connection_manager_from_client(client).await
    }

    pub async fn build_connection_manager_from_client(client: Client) -> Result<ConnectionManager> {
        let redis_cm_config = ConnectionManagerConfig::new()
            .set_connection_timeout(Some(Duration::from_secs(10)))
            .set_response_timeout(Some(Duration::from_secs(5)));
        Ok(ConnectionManager::new_with_config(client, redis_cm_config).await?)
    }
}
