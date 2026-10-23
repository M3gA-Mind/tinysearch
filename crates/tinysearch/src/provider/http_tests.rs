use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Serves one `200` reply: `head` is the header block after the status line,
/// then `body` is written as-is.
async fn serve(head: &'static str, body: Vec<u8>) -> TestResult<String> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = format!("http://{}", listener.local_addr()?);
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut request = [0; 4096];
        let _ = stream.read(&mut request).await;
        let reply = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{head}\r\n");
        let _ = stream.write_all(reply.as_bytes()).await;
        let _ = stream.write_all(&body).await;
    });
    Ok(address)
}

async fn read(address: &str) -> TestResult<Result<Value>> {
    Ok(read_json(reqwest::get(address).await?).await)
}

fn too_large(result: &Result<Value>) -> bool {
    matches!(result, Err(Error::ProviderUnavailable(m)) if m == "provider response too large")
}

fn cap() -> TestResult<usize> {
    Ok(usize::try_from(MAX_BODY_BYTES)?)
}

#[tokio::test]
async fn a_streamed_body_past_the_cap_is_refused() -> TestResult<()> {
    // Chunked, so no Content-Length warns the reader: the cap must stop it.
    let size = cap()? + 1;
    let mut body = format!("{size:x}\r\n").into_bytes();
    body.extend(std::iter::repeat_n(b' ', size));
    body.extend_from_slice(b"\r\n0\r\n\r\n");
    let address = serve("Transfer-Encoding: chunked\r\nConnection: close\r\n", body).await?;

    let result = read(&address).await?;
    assert!(too_large(&result), "{result:?}");
    Ok(())
}

#[tokio::test]
async fn an_announced_length_past_the_cap_is_refused_before_reading() -> TestResult<()> {
    let address = serve(
        "Content-Length: 2000001\r\nConnection: close\r\n",
        Vec::new(),
    )
    .await?;

    let result = read(&address).await?;
    assert!(too_large(&result), "{result:?}");
    Ok(())
}

#[tokio::test]
async fn a_body_at_the_cap_is_read() -> TestResult<()> {
    let mut body = vec![b' '; cap()? - 2];
    body.splice(0..0, *b"{}");
    let address = serve("Connection: close\r\n", body).await?;

    assert_eq!(read(&address).await??, serde_json::json!({}));
    Ok(())
}
