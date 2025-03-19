use actix_web::web::Bytes;
use anyhow::Result;
use actix_ws::Session;
use serde::Deserialize;

use crate::{error::ConnectionClosedError, get_clients};

#[derive(Debug, Clone, Deserialize)]
pub struct IpcEvent {
    pub event: String,
    #[allow(dead_code)]
    pub id: i32,
    pub data: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct UpdatePropertyMessage {
    pub property: String,
    pub value: String,
}

impl UpdatePropertyMessage {
    pub fn new<P, V>(property: P, value: V) -> Self
    where
        P: Into<String>,
        V: Into<String>
    {
        Self { property: property.into(), value: value.into() }
    }

    pub async fn send_to_all(self) -> Result<()> {
        get_clients().lock().await.update(self).await
    }

    pub async fn send_to(self, session: &mut Session) -> Result<()> {
        session.binary(self).await.map_err(|_e| ConnectionClosedError("websocket".into()).into())
    }
}

impl Into<Bytes> for UpdatePropertyMessage {
    fn into(self) -> Bytes {
        let mut message = Vec::new();
        message.append(&mut self.property.as_bytes().to_vec());
        message.push(0u8);
        message.append(&mut self.value.as_bytes().to_vec());
        message.into()
    }
}
