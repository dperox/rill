//! Serve complete bodyless heads without retaining a streaming exchange while idle.
use super::{Transport, dispatch};
use axum::Router;
use http_kit::{Body, FileRegion, Request};
use http_kit_io::{BufResult, Write};
use std::{io, net::SocketAddr};
pub(super) async fn short_request<W: Write + http_kit_io::SendFile>(
    pending: &mut bytes::BytesMut,
    writer: &mut W,
    app: Router,
    peer: SocketAddr,
    transport: Transport,
) -> io::Result<Option<bool>> {
    use axum::http::Method;
    use bytes::Buf;
    use http_kit_h1::proto::{BodyKind, parse_request_head};
    // Parse a copy so delegation can replay the original wire bytes verbatim.
    let mut head = bytes::BytesMut::from(&pending[..]);
    let Ok(Some(request)) = parse_request_head(&mut head, &Default::default()) else {
        return Ok(None);
    };
    if request.body != BodyKind::Empty
        || request.upgrade
        || !matches!(request.parts.method, Method::GET | Method::HEAD)
    {
        return Ok(None);
    }
    pending.advance(pending.len() - head.len());
    let info = request.info();
    let response = dispatch(
        app,
        peer,
        transport,
        Request::from_parts(request.parts, Body::empty()),
    )
    .await;
    send_response(writer, &info, response).await.map(Some)
}

async fn flush<W: Write>(writer: &mut W, chunks: &mut Vec<bytes::Bytes>) -> io::Result<()> {
    if !chunks.is_empty() {
        let BufResult(result, _) = writer.write_all_owned(std::mem::take(chunks)).await;
        result?;
    }
    Ok(())
}

async fn send_response<W: Write + http_kit_io::SendFile>(
    writer: &mut W,
    info: &http_kit_h1::proto::RequestInfo,
    response: axum::response::Response,
) -> io::Result<bool> {
    use axum::body::HttpBody;
    use futures_util::FutureExt;
    use http_body_util::BodyExt;
    let (mut parts, mut body) = response.into_parts();
    let file = parts.extensions.remove::<FileRegion>();
    let length = file
        .as_ref()
        .map(FileRegion::len)
        .or_else(|| body.size_hint().exact());
    let mut headers = bytes::BytesMut::with_capacity(256);
    let date =
        axum::http::HeaderValue::from_str(&httpdate::fmt_http_date(std::time::SystemTime::now()))
            .unwrap();
    let mut plan = http_kit_h1::proto::encode_response_head(
        info,
        &parts,
        length,
        false,
        Some(&date),
        &mut headers,
    )
    .map_err(io::Error::other)?;
    let mut chunks = vec![headers.freeze()];
    if plan.encoder.sends_body() {
        if let Some(file) = file {
            let (file, offset, len) = file.into_parts();
            chunks.extend(plan.encoder.region_start(len).map_err(io::Error::other)?);
            flush(writer, &mut chunks).await?;
            let mut sent = 0;
            while sent < len {
                let count = (len - sent).min(1 << 30) as usize;
                writer
                    .send_file_all(file.clone(), offset + sent, count)
                    .await
                    .0?;
                sent += count as u64;
            }
            chunks.extend(plan.encoder.region_end());
        } else {
            while !body.is_end_stream() {
                let frame = match body.frame().now_or_never() {
                    Some(frame) => frame,
                    None => {
                        flush(writer, &mut chunks).await?;
                        body.frame().await
                    }
                };
                let Some(frame) = frame else {
                    break;
                };
                match frame.map_err(io::Error::other)?.into_data() {
                    Ok(data) => plan
                        .encoder
                        .data(data, &mut chunks)
                        .map_err(io::Error::other)?,
                    Err(frame) => chunks.push(
                        plan.encoder
                            .trailers(&frame.into_trailers().expect("http-body frame"))
                            .map_err(io::Error::other)?,
                    ),
                }
                if chunks.len() >= 8 {
                    flush(writer, &mut chunks).await?;
                }
            }
        }
    }
    chunks.extend(plan.encoder.finish().map_err(io::Error::other)?);
    flush(writer, &mut chunks).await?;
    Ok(plan.keep_alive)
}
