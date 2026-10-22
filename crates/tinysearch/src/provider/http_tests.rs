use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

/// Serves one `200` reply: `head` is the header block after the status line,
/// then `body` is written as-is.
async fn serve(head: &'static str, body: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        let _ = stream.read(&mut request).await;
        let reply = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{head}\r\n");
        let _ = stream.write_all(reply.as_bytes()).await;
        let _ = stream.write_all(&body).await;
    });
    address
}

async fn read(address: &str) -> Result<Value> {
    read_json(reqwest::get(address).await.unwrap()).await
}

fn too_large(result: Result<Value>) -> bool {
    matches!(result, Err(Error::ProviderUnavailable(m)) if m == "provider response too large")
}

#[tokio::test]
async fn a_streamed_body_past_the_cap_is_refused() {
    // Chunked, so no Content-Length warns the reader: the cap must stop it.
    let size = MAX_BODY_BYTES as usize + 1;
    let mut body = format!("{size:x}\r\n").into_bytes();
    body.extend(std::iter::repeat_n(b' ', size));
    body.extend_from_slice(b"\r\n0\r\n\r\n");
    let address = serve("Transfer-Encoding: chunked\r\nConnection: close\r\n", body).await;

    assert!(too_large(read(&address).await));
}

#[tokio::test]
async fn an_announced_length_past_the_cap_is_refused_before_reading() {
    let address = serve(
        "Content-Length: 2000001\r\nConnection: close\r\n",
        Vec::new(),
    )
    .await;

    assert!(too_large(read(&address).await));
}

#[tokio::test]
async fn a_body_at_the_cap_is_read() {
    let size = MAX_BODY_BYTES as usize;
    let mut body = vec![b' '; size - 2];
    body.splice(0..0, *b"{}");
    let address = serve("Connection: close\r\n", body).await;

    assert_eq!(read(&address).await.unwrap(), serde_json::json!({}));
}
