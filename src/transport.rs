use crate::core::RpcName;
use crate::error::{RpcError, RpcResult};

use crate::transport::TransportError::SerialiseError;
use crate::{Bytes, OwnedBytes};
use async_trait::async_trait;
use log::debug;
use serde::{Deserialize, Serialize};
use std::fmt::Formatter;
use std::marker::PhantomData;
use std::time::Duration;

/// Errors specific to transport
#[derive(Debug)]
pub enum TransportError {
    /// Error when sending (from the perspective of the the local program)
    SendError(String),
    /// Error when receiving (from the perspective of the the local program)
    ReceiveError(String),
    /// Error when establishing connection
    ConnectError(String),
    /// Error from timeout after waiting some [Duration].
    ReceiveTimeout(Duration),
    // Error when serialising data
    SerialiseError(String),
    // Error when deserialising data
    DeserialiseError(String),
}
impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::SendError(s) => write!(f, "SendError({})", s),
            TransportError::ReceiveError(s) => write!(f, "ReceiveError({})", s),
            TransportError::ConnectError(s) => write!(f, "ConnectError({})", s),
            TransportError::ReceiveTimeout(dur) => write!(f, "ReceiveTimeout({:?})", dur),
            TransportError::SerialiseError(s) => write!(f, "SerialiseError({})", s),
            TransportError::DeserialiseError(s) => write!(f, "DeserialiseError({})", s),
        }
    }
}
impl std::error::Error for TransportError {}
impl TransportError {
    fn io_send(e: std::io::Error) -> Self {
        Self::SendError(format!("{:?}", e))
    }
    fn io_receive(e: std::io::Error) -> Self {
        Self::ReceiveError(format!("{:?}", e))
    }
}

/// The [InternalTransport] trait defines the transport layer for RPCs between client and server
#[async_trait]
pub trait InternalTransport {
    /// async fn send(&mut self, b: Bytes<'_>) -> Result<(), TransportError>;
    async fn send(&mut self, b: Bytes<'_>) -> Result<(), TransportError>;

    /// async fn send_and_wait_for_response(
    ///     &mut self,
    ///     b: Bytes<'_>,
    ///     timeout: Duration,
    /// ) -> Result<OwnedBytes, TransportError>;
    async fn send_and_wait_for_response(
        &mut self,
        b: Bytes<'_>,
        timeout: Duration,
    ) -> Result<OwnedBytes, TransportError>;

    /// async fn receive(&mut self, timeout: Option<Duration>) -> Result<OwnedBytes, TransportError>;
    async fn receive(&mut self, timeout: Option<Duration>) -> Result<OwnedBytes, TransportError>;
}

#[derive(Serialize, Deserialize)]
struct TransportPackage<'a> {
    #[serde(borrow)]
    name_bytes: Bytes<'a>,
    #[serde(borrow)]
    query_bytes: Bytes<'a>,
}
#[derive(Serialize, Deserialize)]
struct TransportPackageOwned {
    name_bytes: OwnedBytes,
    query_bytes: OwnedBytes,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::HelloWorldRpcName;
    #[test]
    fn transport_package_round_trip() {
        let name = HelloWorldRpcName::HelloWorld;
        let query = String::from("Foo");

        let deo = serde_pickle::DeOptions::new();
        let sero = serde_pickle::SerOptions::new();
        let transport_config = TransportWireConfig::Pickle(deo, sero);

        let name_bytes = transport_config.serialize(&name).unwrap();
        let query_bytes = transport_config.serialize(&query).unwrap();

        let package = TransportPackage {
            name_bytes: &name_bytes,
            query_bytes: &query_bytes,
        };

        let package_bytes = transport_config.serialize(&package).unwrap();

        let package2: TransportPackageOwned = transport_config.deserialize(&package_bytes).unwrap();

        let name2: HelloWorldRpcName = transport_config.deserialize(&package2.name_bytes).unwrap();
        let query2: String = transport_config.deserialize(&package2.query_bytes).unwrap();

        assert_eq!(name, name2);
        assert_eq!(query, query2);
    }
}

/// The initial structure handed to the RpcServer, which includes
pub struct ReceivedQuery<Name: RpcName> {
    pub name: Name,
    pub query_bytes: OwnedBytes,
}

/// Transport for data betweeen client and server, generic over the rpc names and internal transport
/// The majority of the heavy lifting is done by the [internal_transport], see the definition of
/// the [InternalTransport] trait for more information
pub struct Transport<I, Name> {
    internal_transport: I,
    name: PhantomData<Name>,
    pub config: TransportConfig,
}

// TODO: Consider making transport Connected/Disconnected
/*
pub struct ConnectedTransport<I, Name> {
    transport: Transport<I, Name>
}
 */

/// TransportConfig defines various config options for transport handling
/// [rcv_timeout] is used to protect receiving with a timeout
/// [wire_config] is for serialising sent data, see the type def for more
#[derive(Clone, Debug)]
pub struct TransportConfig {
    pub rcv_timeout: Duration,
    pub wire_config: TransportWireConfig,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            rcv_timeout: Duration::from_secs(3),
            wire_config: TransportWireConfig::default(),
        }
    }
}

/// TransportWireConfig defines how to (de)serialise query/response. Extra methods are available by enabling their feature
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum TransportWireConfig {
    Pickle(serde_pickle::DeOptions, serde_pickle::SerOptions),
    #[cfg(feature = "transport_postcard")]
    Postcard,
}

// TODO: Handle unwraps here with some sort of [Serialise/DeserialiseError]
impl TransportWireConfig {
    pub(crate) fn serialize(&self, val: &impl Serialize) -> Result<OwnedBytes, TransportError> {
        match self {
            Self::Pickle(_de_opts, ser_opts) => serde_pickle::ser::to_vec(val, ser_opts.clone())
                .map_err(|pickle_error| SerialiseError(format!("{:?}", pickle_error))),
            #[cfg(feature = "transport_postcard")]
            Self::Postcard => postcard::to_vec(val)
                .map_err(|postcard_error| SerialiseError(format!("{:?}", postcard_error))),
        }
    }
    pub(crate) fn deserialize<T: for<'de> Deserialize<'de>>(
        &self,
        bytes: Bytes,
    ) -> Result<T, TransportError> {
        match self {
            Self::Pickle(de_opts, _ser_opts) => {
                serde_pickle::de::from_slice(bytes, de_opts.clone()).map_err(|pickle_error| {
                    TransportError::DeserialiseError(format!("{:?}", pickle_error))
                })
            }
            #[cfg(feature = "transport_postcard")]
            Self::Postcard => postcard::from_bytes(bytes).map_err(|postcard_error| {
                TransportError::DeserialiseError(format!("{:?}", postcard_error))
            }),
        }
    }
}

impl Default for TransportWireConfig {
    fn default() -> Self {
        Self::Pickle(
            serde_pickle::DeOptions::new(),
            serde_pickle::SerOptions::new(),
        )
    }
}

impl<I: InternalTransport, Name: RpcName> Transport<I, Name> {
    pub fn new(internal_transport: I, transport_config: TransportConfig) -> Self {
        Self {
            internal_transport,
            name: PhantomData::default(),
            config: transport_config,
        }
    }
    pub async fn send_query(
        &mut self,
        query_bytes: Bytes<'_>,
        rpc_name: &Name,
    ) -> RpcResult<OwnedBytes> {
        let name_bytes = self.config.wire_config.serialize(&rpc_name)?;
        let package = TransportPackage {
            name_bytes: &name_bytes,
            query_bytes,
        };
        let package_bytes = self.config.wire_config.serialize(&package)?;
        debug!(
            "Transport sending {} Bytes:  {:?}",
            package_bytes.len(),
            package_bytes
        );
        self.internal_transport
            .send_and_wait_for_response(&package_bytes, self.config.rcv_timeout)
            .await
            .map_err(Into::into)
    }

    pub async fn receive_query(&mut self) -> RpcResult<ReceivedQuery<Name>> {
        // We receive with no timeout as we want to sit and wait on [internal_transport]
        match self.internal_transport.receive(None).await {
            Ok(bytes) => {
                debug!("Transport {} Bytes:  {:?}", bytes.len(), bytes);
                let package: TransportPackageOwned = self.config.wire_config.deserialize(&bytes)?;
                let name = self.config.wire_config.deserialize(&package.name_bytes)?;
                Ok(ReceivedQuery {
                    name,
                    query_bytes: package.query_bytes,
                })
            }
            Err(rpc_error) => Err(RpcError::TransportError(rpc_error)),
        }
    }

    pub async fn respond(&mut self, bytes: Bytes<'_>) -> RpcResult<()> {
        self.internal_transport
            .send(bytes)
            .await
            .map_err(RpcError::TransportError)
    }
}

#[cfg(test)]
pub(crate) struct CannedTestingTransport {
    pub always_respond_with: String,
    pub receive_times: usize,
}

#[cfg(test)]
#[async_trait]
impl InternalTransport for CannedTestingTransport {
    async fn send(&mut self, _b: Bytes<'_>) -> Result<(), TransportError> {
        Ok(())
    }

    async fn send_and_wait_for_response(
        &mut self,
        _b: Bytes<'_>,
        _timeout: Duration,
    ) -> Result<OwnedBytes, TransportError> {
        Ok(
            serde_pickle::to_vec(&self.always_respond_with, serde_pickle::SerOptions::new())
                .unwrap(),
        )
    }
    async fn receive(&mut self, _timeout: Option<Duration>) -> Result<OwnedBytes, TransportError> {
        if self.receive_times > 0 {
            self.receive_times -= 1;
            Ok(
                serde_pickle::to_vec(&self.always_respond_with, serde_pickle::SerOptions::new())
                    .unwrap(),
            )
        } else {
            Err(TransportError::ReceiveError(String::from(
                "Run out of receive count",
            )))
        }
    }
}

/// Pre-packaged implementation of [InternalTransport] using [tokio::net::TcpStream]
pub struct TcpTransport {
    stream: tokio::net::TcpStream,
}

impl TcpTransport {
    pub fn new(stream: tokio::net::TcpStream) -> Self {
        Self { stream }
    }
}

#[async_trait]
impl InternalTransport for TcpTransport {
    async fn send(&mut self, b: Bytes<'_>) -> Result<(), TransportError> {
        use tokio::io::AsyncWriteExt;
        self.stream
            .write_all(b)
            .await
            .map_err(TransportError::io_send)
    }

    async fn send_and_wait_for_response(
        &mut self,
        b: Bytes<'_>,
        timeout: Duration,
    ) -> Result<OwnedBytes, TransportError> {
        self.send(b).await?;
        self.receive(Some(timeout)).await
    }

    async fn receive(&mut self, timeout: Option<Duration>) -> Result<OwnedBytes, TransportError> {
        use tokio::io::AsyncReadExt;
        // 1024 * 8 = 8192 bits = 256 * u32s
        let mut buf = [0u8; 1024];
        let mut return_bytes = Vec::new();
        loop {
            let read_fut = self.stream.read(&mut buf);
            let result = match timeout {
                Some(timeout_) => match tokio::time::timeout(timeout_, read_fut).await {
                    Ok(r) => r,
                    Err(_) => return Err(TransportError::ReceiveTimeout(timeout_)),
                },
                None => read_fut.await,
            };
            match result {
                Ok(0) => {
                    return Ok(return_bytes);
                }
                Ok(bytes_received) => {
                    return_bytes.extend_from_slice(&buf[0..bytes_received]);
                    if bytes_received < buf.len() {
                        return Ok(return_bytes);
                    }
                }
                Err(e) => {
                    return Err(TransportError::io_receive(e));
                }
            };
        }
    }
}
