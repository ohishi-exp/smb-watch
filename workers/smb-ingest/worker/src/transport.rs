//! smb2 の `TransportFactory`: Workers の TCP socket 上で SMB2 の direct-TCP framing を話す。
//!
//! ohishi-exp/smb2 の `examples/workers-probe/src/lib.rs` (`WorkerSockets`) を写し、socket の取得だけを
//! 差し替えている: 本番は Workers VPC の binding (`SMB_VPC`) の `connect()`、ローカル検証は
//! var `LOCAL_SMB_ADDR` があるときだけ `Socket::builder().connect` で直接繋ぐ。

use async_trait::async_trait;
use smb2::transport::{TransportFactory, TransportReceive, TransportSend};
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::Mutex;
use worker::send::{SendFuture, SendWrapper};
use worker::Socket;

use crate::tcp::TcpPort;

/// VPC Service 型は宛先の host:port を Service 側で固定するので、`connect()` に渡すアドレスは名目の値
/// (この文字列は使われない。社内のアドレスはコードに書かない)。
pub(crate) const VPC_NOMINAL_ADDR: &str = "smb:445";

/// SMB 接続の開き方。
pub(crate) enum Route {
    /// 本番: Workers VPC の binding。
    Vpc(TcpPort),
    /// ローカル検証: `LOCAL_SMB_ADDR` (host:port) へ直接。
    Direct,
}

pub(crate) struct WorkerSockets(SendWrapper<Route>);

impl WorkerSockets {
    pub(crate) fn new(route: Route) -> Self {
        Self(SendWrapper::new(route))
    }
}

// smb2 の `ClientConfig` が `Debug` なので要る。binding の中身は出さない。
impl std::fmt::Debug for WorkerSockets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self.0 {
            Route::Vpc(_) => f.write_str("WorkerSockets(vpc)"),
            Route::Direct => f.write_str("WorkerSockets(direct)"),
        }
    }
}

#[async_trait]
impl TransportFactory for WorkerSockets {
    async fn connect(
        &self,
        addr: &str,
    ) -> smb2::Result<(Box<dyn TransportSend>, Box<dyn TransportReceive>)> {
        // A JS socket isn't `Send`; a Worker has one thread, so wrapping is
        // sound, and smb2's trait objects need it.
        let socket = SendFuture::new(async move {
            let socket = match &*self.0 {
                Route::Vpc(vpc) => Socket::from(
                    vpc.connect(VPC_NOMINAL_ADDR)
                        .map_err(|e| worker::Error::from(format!("vpc connect: {e:?}")))?,
                ),
                Route::Direct => {
                    let (host, port) = split_host_port(addr)
                        .ok_or_else(|| worker::Error::from("LOCAL_SMB_ADDR is not host:port"))?;
                    Socket::builder().connect(host, port)?
                }
            };
            // Surface a refused connect here rather than on the first write.
            socket.opened().await?;
            Ok::<_, worker::Error>(socket)
        })
        .await
        .map_err(|e| smb2::Error::Io(std::io::Error::other(e.to_string())))?;
        let (reader, writer) = tokio::io::split(socket);
        Ok((
            Box::new(SocketSend(Mutex::new(SendWrapper::new(writer)))),
            Box::new(SocketReceive(Mutex::new(SendWrapper::new(reader)))),
        ))
    }
}

fn split_host_port(addr: &str) -> Option<(String, u16)> {
    let (host, port) = addr.rsplit_once(':')?;
    let port = port.parse::<u16>().ok()?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    Some((host.to_string(), port))
}

/// The largest message the 3-byte length in the direct-TCP header can carry
/// (MS-SMB2 § 2.1).
const MAX_FRAME: usize = (1 << 24) - 1;

struct SocketSend(Mutex<SendWrapper<WriteHalf<Socket>>>);
struct SocketReceive(Mutex<SendWrapper<ReadHalf<Socket>>>);

#[async_trait]
impl TransportSend for SocketSend {
    async fn send(&self, data: &[u8]) -> smb2::Result<()> {
        if data.len() > MAX_FRAME {
            return Err(smb2::Error::invalid_data(format!(
                "message of {} bytes doesn't fit a frame",
                data.len()
            )));
        }
        // One write per frame, header included: a zero byte, then the length
        // in 3 bytes big-endian.
        let mut frame = Vec::with_capacity(4 + data.len());
        frame.extend_from_slice(&(data.len() as u32).to_be_bytes());
        frame.extend_from_slice(data);
        SendFuture::new(async {
            let mut writer = self.0.lock().await;
            writer.write_all(&frame).await?;
            writer.flush().await
        })
        .await
        .map_err(smb2::Error::Io)
    }
}

#[async_trait]
impl TransportReceive for SocketReceive {
    async fn receive(&self) -> smb2::Result<Vec<u8>> {
        SendFuture::new(async {
            let mut reader = self.0.lock().await;
            let mut header = [0u8; 4];
            reader.read_exact(&mut header).await.map_err(read_error)?;
            if header[0] != 0 {
                return Err(smb2::Error::invalid_data(format!(
                    "bad direct-TCP header {header:02x?}"
                )));
            }
            let mut message = vec![0u8; u32::from_be_bytes(header) as usize];
            reader.read_exact(&mut message).await.map_err(read_error)?;
            Ok(message)
        })
        .await
    }
}

/// The peer closing the socket is a disconnect, as smb2's own TCP reports it.
fn read_error(e: std::io::Error) -> smb2::Error {
    if e.kind() == std::io::ErrorKind::UnexpectedEof {
        smb2::Error::Disconnected
    } else {
        smb2::Error::Io(e)
    }
}
