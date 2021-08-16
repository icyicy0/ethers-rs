use crate::{
    bundle::BundleHash,
    jsonrpc::{JsonRpcError, Request, Response},
};
use ethers_core::{types::H256, utils::keccak256};
use ethers_signers::Signer;
use reqwest::{Client, Error as ReqwestError};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;
use url::Url;

/// A Flashbots relay client.
///
/// The client automatically signs every request and sets the Flashbots
/// authorization header appropriately with the given signer.
///
/// **Note**: You probably do not want to use this directly, unless
/// you want to interact directly with the Relay. Most users should use
/// [`FlashbotsMiddleware`](crate::FlashbotsMiddleware) instead.
#[derive(Debug)]
pub struct Relay<S> {
    id: AtomicU64,
    client: Client,
    url: Url,
    signer: S,
}

/// Errors for relay requests.
#[derive(Error, Debug)]
pub enum RelayError<S: Signer> {
    /// The request failed.
    #[error(transparent)]
    RequestError(#[from] ReqwestError),
    /// The request could not be parsed.
    #[error(transparent)]
    JsonRpcError(#[from] JsonRpcError),
    /// The request could not be serialized.
    #[error(transparent)]
    RequestSerdeJson(#[from] serde_json::Error),
    /// The request could not be signed.
    #[error(transparent)]
    SignerError(#[from(S::Error)] S::Error),
    /// The response could not be deserialized.
    #[error("Deserialization error: {err}. Response: {text}")]
    ResponseSerdeJson {
        err: serde_json::Error,
        text: String,
    },
}

impl<S: Signer> Relay<S> {
    /// Initializes a new relay client.
    pub fn new(url: impl Into<Url>, signer: S) -> Self {
        Self {
            id: AtomicU64::new(0),
            client: Client::new(),
            url: url.into(),
            signer,
        }
    }

    /// Sends a request with the provided method to the relay, with the
    /// parameters serialized as JSON.
    pub async fn request<T: Serialize + Send + Sync, R: DeserializeOwned>(
        &self,
        method: &str,
        params: T,
    ) -> Result<R, RelayError<S>> {
        let next_id = self.id.load(Ordering::SeqCst) + 1;
        self.id.store(next_id, Ordering::SeqCst);

        let payload = Request::new(next_id, method, params);

        let signature = self
            .signer
            .sign_message(format!(
                "0x{:x}",
                H256::from(keccak256(
                    serde_json::to_string(&payload)
                        .map_err(RelayError::RequestSerdeJson)?
                        .as_bytes()
                ))
            ))
            .await
            .map_err(RelayError::SignerError)?;

        let res = self
            .client
            .post(self.url.as_ref())
            .header(
                "X-Flashbots-Signature",
                format!("{:?}:0x{}", self.signer.address(), signature),
            )
            .json(&payload)
            .send()
            .await?;
        let text = res.text().await?;
        let res: Response<R> = serde_json::from_str(&text)
            .map_err(|err| RelayError::ResponseSerdeJson { err, text })?;

        Ok(res.data.into_result()?)
    }
}

impl<S: Signer + Clone> Clone for Relay<S> {
    fn clone(&self) -> Self {
        Self {
            id: AtomicU64::new(0),
            client: self.client.clone(),
            url: self.url.clone(),
            signer: self.signer.clone(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SendBundleResponse {
    pub(crate) bundle_hash: BundleHash,
}