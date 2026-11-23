//! The engine bridge's local socket: a Unix domain socket where there is one. Other targets
//! have no bridge yet and say so when it is started.

use std::path::Path;

use tokio::io::{AsyncRead, AsyncWrite};

/// One accepted connection.
pub trait LocalStream: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> LocalStream for T {}

pub struct LocalListener {
    #[cfg(unix)]
    inner: tokio::net::UnixListener,
}

impl LocalListener {
    /// Bind `path`, replacing a socket left by an earlier run, with owner-only access: the
    /// socket accepts commands that pop windows over the user's screen.
    pub fn bind(path: &Path) -> anyhow::Result<Self> {
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(path);
            let inner = tokio::net::UnixListener::bind(path)?;
            super::files::restrict_to_owner(path)?;
            Ok(Self { inner })
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Err(super::unsupported::unsupported("The engine bridge"))
        }
    }

    pub async fn accept(&self) -> anyhow::Result<Box<dyn LocalStream>> {
        #[cfg(unix)]
        {
            let (stream, _) = self.inner.accept().await?;
            Ok(Box::new(stream))
        }
        #[cfg(not(unix))]
        {
            Err(super::unsupported::unsupported("The engine bridge"))
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn accepts_a_connection_on_an_owner_only_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bridge.sock");
        let rt = tokio::runtime::Builder::new_current_thread().enable_io().build().unwrap();
        rt.block_on(async {
            let listener = LocalListener::bind(&path).unwrap();
            assert_eq!(super::super::files::is_owner_only(&path), Some(true));
            let client = tokio::spawn({
                let path = path.clone();
                async move {
                    let mut s = tokio::net::UnixStream::connect(path).await.unwrap();
                    s.write_all(b"hi").await.unwrap();
                }
            });
            let mut stream = listener.accept().await.unwrap();
            let mut buf = [0u8; 2];
            stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"hi");
            client.await.unwrap();
        });
    }
}
