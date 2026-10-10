use anyhow::{Context, Result, bail};
use bytes::Bytes;
use futures_util::TryStreamExt;
use http_body_util::{BodyExt, Full, StreamBody, combinators::UnsyncBoxBody};
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Frame, Incoming},
    header::{self, HeaderValue},
    service::service_fn,
};
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::{TokioExecutor, TokioIo},
};
use std::{
    convert::Infallible,
    net::{Ipv4Addr, TcpListener},
    path::{Component, Path, PathBuf},
    sync::{Arc, mpsc},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    runtime::Runtime,
    sync::watch,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type Body = UnsyncBoxBody<Bytes, BoxError>;
type HttpClient = Client<hyper_rustls::HttpsConnector<HttpConnector>, Body>;

fn body(value: impl Into<Bytes>) -> Body {
    Full::new(value.into())
        .map_err(|never| match never {})
        .boxed_unsync()
}
fn failure(status: StatusCode, message: &str) -> Response<Body> {
    let mut response = Response::new(body(message.to_owned()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

pub struct Server {
    pub port: u16,
    cancel: watch::Sender<bool>,
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.cancel.send(true);
    }
}

#[derive(Clone)]
enum Host {
    Files(Arc<PathBuf>),
    Relay {
        target: url::Url,
        client: HttpClient,
    },
}

/// Created only while projects are starting/running. Dropping this releases its worker and sockets.
pub struct Network {
    runtime: Option<Runtime>,
    client: HttpClient,
}
impl Network {
    pub fn new() -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .thread_name("project-network")
            .build()?;
        let tls = hyper_rustls::HttpsConnectorBuilder::new()
            .with_provider_and_native_roots(rustls::crypto::ring::default_provider())?
            .https_or_http()
            .enable_http1()
            .build();
        let client = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(Duration::from_secs(10))
            .build(tls);
        Ok(Self {
            runtime: Some(runtime),
            client,
        })
    }

    pub fn files(&self, directory: &Path) -> Result<Server> {
        self.host(
            "127.0.0.1",
            Host::Files(Arc::new(std::fs::canonicalize(directory)?)),
        )
    }

    pub fn relay(&self, target: &str) -> Result<Server> {
        let target = loopback_url(target)?;
        self.host(
            "0.0.0.0",
            Host::Relay {
                target,
                client: self.client.clone(),
            },
        )
    }

    fn host(&self, address: &str, host: Host) -> Result<Server> {
        let listener = TcpListener::bind((address, 0)).context("无法启动项目预览服务")?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let (cancel, mut stopped) = watch::channel(false);
        let runtime = self.runtime.as_ref().unwrap();
        let listener = {
            let _entered = runtime.enter();
            tokio::net::TcpListener::from_std(listener)?
        };
        runtime.spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    _ = stopped.changed() => break,
                    Some(_) = connections.join_next(), if !connections.is_empty() => {},
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break };
                        let host = host.clone();
                        let cancelled = stopped.clone();
                        connections.spawn(async move {
                            let mut shutdown = cancelled.clone();
                            let service = service_fn(move |request| {
                                let host = host.clone();
                                let cancel = cancelled.clone();
                                async move {
                                    let response = match host {
                                        Host::Files(root) => files(request, &root).await,
                                        Host::Relay { target, client } => relay(request, target, client, cancel).await,
                                    }.unwrap_or_else(|_| failure(StatusCode::BAD_GATEWAY, "Project preview is unavailable."));
                                    Ok::<_, Infallible>(response)
                                }
                            });
                            // A separate receiver cancels every active connection on project stop.
                            let connection = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(stream), service).with_upgrades();
                            tokio::select! { _ = connection => {}, _ = shutdown.changed() => {} }
                        });
                    }
                }
            }
            connections.abort_all();
        });
        Ok(Server { port, cancel })
    }

    pub fn probe(&self, target: &str) -> Result<mpsc::Receiver<bool>> {
        let target = loopback_url(target)?;
        let client = self.client.clone();
        let (tx, rx) = mpsc::channel();
        self.runtime.as_ref().unwrap().spawn(async move {
            let request = Request::builder()
                .uri(target.as_str())
                .body(body(Bytes::new()));
            let ready = match request {
                Ok(request) => {
                    tokio::time::timeout(Duration::from_secs(2), client.request(request))
                        .await
                        .is_ok_and(|result| {
                            result.is_ok_and(|response| response.status().is_success())
                        })
                }
                Err(_) => false,
            };
            let _ = tx.send(ready);
        });
        Ok(rx)
    }
}

impl Drop for Network {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

fn loopback_url(value: &str) -> Result<url::Url> {
    let target = url::Url::parse(value).context("项目没有提供有效的本机预览地址")?;
    if !matches!(target.scheme(), "http" | "https")
        || !matches!(target.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
        || !target.username().is_empty()
        || target.password().is_some()
    {
        bail!("项目预览地址必须是本机 HTTP 地址");
    }
    Ok(target)
}

async fn files(request: Request<Incoming>, root: &Path) -> Result<Response<Body>> {
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return Ok(failure(StatusCode::METHOD_NOT_ALLOWED, "Use GET or HEAD."));
    }
    let decoded = percent_encoding::percent_decode_str(request.uri().path()).decode_utf8()?;
    let relative = Path::new(decoded.trim_start_matches('/'));
    if decoded.contains(['\\', ':', '\0'])
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || relative
            .components()
            .any(|component| component.as_os_str().to_string_lossy().starts_with('.'))
    {
        return Ok(failure(StatusCode::FORBIDDEN, "Invalid file path."));
    }
    let mut file = root.join(relative);
    if tokio::fs::metadata(&file)
        .await
        .is_ok_and(|metadata| metadata.is_dir())
    {
        file.push("index.html");
    }
    let Ok(file) = tokio::fs::canonicalize(file).await else {
        return Ok(failure(StatusCode::NOT_FOUND, "File not found."));
    };
    if !file.starts_with(root) {
        return Ok(failure(StatusCode::FORBIDDEN, "Invalid file path."));
    }
    let mut opened = tokio::fs::File::open(&file).await?;
    let length = opened.metadata().await?.len();
    let mut start = 0;
    let mut end = length.saturating_sub(1);
    let mut partial = false;
    if let Some(range) = request.headers().get(header::RANGE) {
        let parsed = range
            .to_str()
            .ok()
            .and_then(|value| value.strip_prefix("bytes="))
            .and_then(|value| value.split_once('-'))
            .and_then(|(first, last)| {
                if first.is_empty() {
                    let suffix: u64 = last.parse().ok()?;
                    (suffix > 0)
                        .then_some((length.saturating_sub(suffix), length.saturating_sub(1)))
                } else {
                    Some((
                        first.parse::<u64>().ok()?,
                        if last.is_empty() {
                            length.saturating_sub(1)
                        } else {
                            last.parse::<u64>().ok()?.min(length.saturating_sub(1))
                        },
                    ))
                }
            });
        let Some((first, last)) =
            parsed.filter(|(first, last)| length > 0 && first <= last && *first < length)
        else {
            let mut response = failure(StatusCode::RANGE_NOT_SATISFIABLE, "Invalid byte range.");
            response
                .headers_mut()
                .insert(header::CONTENT_RANGE, format!("bytes */{length}").parse()?);
            return Ok(response);
        };
        start = first;
        end = last;
        partial = true;
    }
    let size = if length == 0 { 0 } else { end - start + 1 };
    let payload = if request.method() == Method::HEAD {
        body(Bytes::new())
    } else {
        opened.seek(std::io::SeekFrom::Start(start)).await?;
        let stream = tokio_util::io::ReaderStream::new(opened.take(size))
            .map_ok(Frame::data)
            .map_err(|error| Box::new(error) as BoxError);
        BodyExt::boxed_unsync(StreamBody::new(stream))
    };
    let mut response = Response::new(payload);
    *response.status_mut() = if partial {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, size.to_string().parse()?);
    response
        .headers_mut()
        .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        mime_guess::from_path(file)
            .first_or_octet_stream()
            .as_ref()
            .parse()?,
    );
    if partial {
        response.headers_mut().insert(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{length}").parse()?,
        );
    }
    Ok(response)
}

fn remove_hop_headers(headers: &mut hyper::HeaderMap, upgrade: bool) {
    if !upgrade {
        if let Some(names) = headers
            .get(header::CONNECTION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
        {
            for name in names.split(',') {
                headers.remove(name.trim());
            }
        }
        headers.remove(header::CONNECTION);
        headers.remove(header::UPGRADE);
    }
    for name in [
        "proxy-connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
    ] {
        headers.remove(name);
    }
}

async fn relay(
    mut request: Request<Incoming>,
    target: url::Url,
    client: HttpClient,
    mut cancel: watch::Receiver<bool>,
) -> Result<Response<Body>> {
    let upstream = target.origin().ascii_serialization();
    let incoming = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(|host| format!("http://{host}"));
    let upgrade = request.headers().get(header::UPGRADE).is_some();
    let downstream_upgrade = upgrade.then(|| hyper::upgrade::on(&mut request));
    let path = request
        .uri()
        .path_and_query()
        .map_or("/", |value| value.as_str());
    *request.uri_mut() = format!("{upstream}{path}").parse()?;
    request.headers_mut().insert(
        header::HOST,
        target[url::Position::BeforeHost..url::Position::AfterPort].parse()?,
    );
    for name in [header::ORIGIN, header::REFERER] {
        if let Some(value) = request
            .headers()
            .get(&name)
            .and_then(|value| value.to_str().ok())
        {
            if let Some(incoming) = &incoming {
                if value.starts_with(incoming) {
                    let rewritten = format!("{upstream}{}", &value[incoming.len()..]);
                    request.headers_mut().insert(name, rewritten.parse()?);
                }
            }
        }
    }
    remove_hop_headers(request.headers_mut(), upgrade);
    let request = request.map(|body| {
        body.map_err(|error| Box::new(error) as BoxError)
            .boxed_unsync()
    });
    let mut response =
        tokio::time::timeout(Duration::from_secs(60), client.request(request)).await??;
    let switching = response.status() == StatusCode::SWITCHING_PROTOCOLS;
    if switching {
        if let Some(downstream) = downstream_upgrade {
            let upstream = hyper::upgrade::on(&mut response);
            tokio::spawn(async move {
                tokio::select! {
                    _ = cancel.changed() => {},
                    _ = async {
                        if let (Ok(downstream), Ok(upstream)) = tokio::join!(downstream, upstream) {
                            let _ = tokio::io::copy_bidirectional(&mut TokioIo::new(downstream), &mut TokioIo::new(upstream)).await;
                        }
                    } => {},
                }
            });
        }
    }
    for name in [header::LOCATION, header::ACCESS_CONTROL_ALLOW_ORIGIN] {
        if let Some(value) = response
            .headers()
            .get(&name)
            .and_then(|value| value.to_str().ok())
        {
            if let (Some(suffix), Some(incoming)) = (value.strip_prefix(&upstream), &incoming) {
                let rewritten = format!("{incoming}{suffix}");
                response.headers_mut().insert(name, rewritten.parse()?);
            }
        }
    }
    remove_hop_headers(response.headers_mut(), switching);
    Ok(response.map(|body| {
        body.map_err(|error| Box::new(error) as BoxError)
            .boxed_unsync()
    }))
}

/// Read actual physical Ethernet/Wi-Fi adapters, excluding VPN and virtual-switch addresses.
pub fn lan_addresses() -> Vec<Ipv4Addr> {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::{
            Foundation::ERROR_BUFFER_OVERFLOW,
            NetworkManagement::{IpHelper::*, Ndis::IfOperStatusUp},
            Networking::WinSock::{AF_INET, SOCKADDR_IN},
        };
        let mut size = 16 * 1024u32;
        for _ in 0..3 {
            let mut memory = vec![0u64; (size as usize).div_ceil(8)];
            let first = memory.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
            let result = GetAdaptersAddresses(
                AF_INET as u32,
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
                std::ptr::null(),
                first,
                &mut size,
            );
            if result == ERROR_BUFFER_OVERFLOW {
                continue;
            }
            if result != 0 {
                return Vec::new();
            }
            let mut addresses = Vec::new();
            let mut adapter = first;
            while !adapter.is_null() {
                let current = &*adapter;
                let mut row = MIB_IF_ROW2 {
                    InterfaceLuid: current.Luid,
                    ..Default::default()
                };
                if current.OperStatus == IfOperStatusUp
                    && matches!(current.IfType, 6 | 71)
                    && GetIfEntry2(&mut row) == 0
                    && row.InterfaceAndOperStatusFlags._bitfield & 1 != 0
                {
                    let mut unicast = current.FirstUnicastAddress;
                    while !unicast.is_null() {
                        let socket = (*unicast).Address.lpSockaddr;
                        if !socket.is_null() && (*socket).sa_family == AF_INET {
                            let socket = &*socket.cast::<SOCKADDR_IN>();
                            let ip = Ipv4Addr::from(socket.sin_addr.S_un.S_addr.to_ne_bytes());
                            if ip.is_private() && !addresses.contains(&ip) {
                                addresses.push(ip);
                            }
                        }
                        unicast = (*unicast).Next;
                    }
                }
                adapter = current.Next;
            }
            addresses.sort();
            return addresses;
        }
    }
    Vec::new()
}
