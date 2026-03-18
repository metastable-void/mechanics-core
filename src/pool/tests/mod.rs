use super::*;
use crate::{
    EndpointBodyType, HttpEndpoint, HttpMethod, MechanicsConfig, QuerySpec, SlottedQueryMode,
    UrlParamSpec,
};
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Barrier;

fn make_job(source: &str, config: MechanicsConfig, arg: Value) -> MechanicsJob {
    MechanicsJob {
        mod_source: Arc::<str>::from(source),
        arg: Arc::new(arg),
        config: Arc::new(config),
    }
}

fn http_status_reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Status",
    }
}

fn spawn_json_server_with_status(
    delay: Duration,
    status: u16,
    response_json: &'static str,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
    let addr = listener.local_addr().expect("read local addr");
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept one connection");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set read timeout");

        let mut buf = [0_u8; 4096];
        let _ = stream.read(&mut buf);
        if !delay.is_zero() {
            thread::sleep(delay);
        }

        let body = response_json.as_bytes();
        let response = format!(
            "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            http_status_reason(status),
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .expect("write headers");
        stream.write_all(body).expect("write body");
        let _ = stream.flush();
    });

    (format!("http://{addr}"), handle)
}

fn spawn_json_server_with_status_owned(
    delay: Duration,
    status: u16,
    response_json: String,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
    let addr = listener.local_addr().expect("read local addr");
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept one connection");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set read timeout");

        let mut buf = [0_u8; 4096];
        let _ = stream.read(&mut buf);
        if !delay.is_zero() {
            thread::sleep(delay);
        }

        let body = response_json.as_bytes();
        let response = format!(
            "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            http_status_reason(status),
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .expect("write headers");
        stream.write_all(body).expect("write body");
        let _ = stream.flush();
    });

    (format!("http://{addr}"), handle)
}

fn spawn_json_server(
    delay: Duration,
    response_json: &'static str,
) -> (String, thread::JoinHandle<()>) {
    spawn_json_server_with_status(delay, 200, response_json)
}

fn spawn_json_server_owned(
    delay: Duration,
    response_json: String,
) -> (String, thread::JoinHandle<()>) {
    spawn_json_server_with_status_owned(delay, 200, response_json)
}

fn endpoint_config(name: &str, endpoint: HttpEndpoint) -> MechanicsConfig {
    let mut endpoints = HashMap::new();
    endpoints.insert(name.to_owned(), endpoint);
    MechanicsConfig::new(endpoints).expect("create config")
}

fn synthetic_pool(
    queue_capacity: usize,
    execution_limits: MechanicsExecutionLimits,
) -> MechanicsPool {
    let (tx, rx) = bounded(queue_capacity);
    let (exit_tx, exit_rx) = bounded(8);
    let shared = Arc::new(MechanicsPoolShared {
        tx,
        rx,
        exit_tx,
        exit_rx,
        workers: RwLock::new(HashMap::new()),
        next_worker_id: AtomicUsize::new(0),
        desired_worker_count: 1,
        closed: AtomicBool::new(false),
        restart_blocked: AtomicBool::new(false),
        restart_guard: Mutex::new(RestartGuard::new(Duration::from_secs(1), 1)),
        execution_limits,
        default_http_timeout_ms: None,
        default_http_response_max_bytes: None,
        reqwest_client: reqwest::Client::new(),
        #[cfg(test)]
        force_worker_runtime_init_failure: false,
    });

    MechanicsPool {
        shared,
        enqueue_timeout: Duration::from_millis(10),
        run_timeout: Duration::from_millis(50),
        supervisor: None,
    }
}

fn is_transient_internet_transport_error(msg: &str) -> bool {
    let msg = msg.to_ascii_lowercase();
    msg.contains("error sending request")
        || msg.contains("dns error")
        || msg.contains("failed to lookup address")
        || msg.contains("connection refused")
        || msg.contains("connection reset")
        || msg.contains("network is unreachable")
        || msg.contains("tls")
        || msg.contains("certificate")
}

fn run_internet_job_with_retry(
    pool: &MechanicsPool,
    job: &MechanicsJob,
    test_name: &str,
) -> Option<Result<Value, MechanicsError>> {
    const ATTEMPTS: usize = 3;
    for attempt in 1..=ATTEMPTS {
        let result = pool.run(job.clone());
        match &result {
            Err(MechanicsError::Execution(msg)) if is_transient_internet_transport_error(msg) => {
                if attempt < ATTEMPTS {
                    thread::sleep(Duration::from_millis(200));
                    continue;
                }
                eprintln!(
                    "skipping {test_name}: transient internet transport error after {ATTEMPTS} attempts: {msg}"
                );
                return None;
            }
            _ => return Some(result),
        }
    }
    None
}

mod endpoint_network;
mod endpoint_validation;
mod internet;
mod lifecycle;
mod queue;
mod runtime_behavior;
mod synthetic_modules;
