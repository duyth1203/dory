use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dory_core::domain::FailureClass;
use dory_core::models::{Availability, Llm, LlmCaps, ModelId, Unavailable, UnavailableReason};
use dory_core::ports::{
    CancelToken, Classify, CompletionRequest, EngineError, LanguageModel, ModelProvider,
    ProvisionError, ProvisionSink, TextSink,
};
use dory_engines::Ollama;
use serde_json::json;

const CHAT_STREAM: &str = include_str!("fixtures/ollama/chat_stream.ndjson");
const CHAT_ERROR: &str = include_str!("fixtures/ollama/chat_error_midstream.ndjson");
const CHAT_NOT_FOUND: &str = include_str!("fixtures/ollama/chat_model_not_found.json");
const TAGS_WITH: &str = include_str!("fixtures/ollama/tags_with_model.json");
const TAGS_WITHOUT: &str = include_str!("fixtures/ollama/tags_without_model.json");
const PULL_STREAM: &str = include_str!("fixtures/ollama/pull_stream.ndjson");
const PULL_NOT_FOUND: &str = include_str!("fixtures/ollama/pull_not_found.ndjson");
const PULL_STALLED_HEAD: &str = include_str!("fixtures/ollama/pull_stalled_head.ndjson");

struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}

struct Server {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
}

impl Server {
    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn bodies(&self, path: &str) -> Vec<serde_json::Value> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path == path)
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect()
    }

    fn requests(&self) -> Vec<(String, String)> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|r| (r.method.clone(), r.path.clone()))
            .collect()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
    }
}

fn read_request(stream: &TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();
    let mut length = 0usize;
    loop {
        line.clear();
        reader.read_line(&mut line).ok()?;
        let header = line.trim();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().ok()?;
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    Some(Request { method, path, body })
}

fn serve(handler: impl Fn(&Request, &mut TcpStream) + Send + Sync + 'static) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let handler = Arc::new(handler);
    {
        let seen = Arc::clone(&seen);
        let stop = Arc::clone(&stop);
        thread::spawn(move || {
            for stream in listener.incoming() {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                let seen = Arc::clone(&seen);
                let handler = Arc::clone(&handler);
                thread::spawn(move || {
                    let Some(request) = read_request(&stream) else {
                        return;
                    };
                    handler(&request, &mut stream);
                    seen.lock().unwrap().push(request);
                });
            }
        });
    }
    Server { addr, seen, stop }
}

fn send_json(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = if status == 200 { "OK" } else { "Error" };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
}

fn chunked_head(stream: &mut TcpStream) {
    stream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n")
        .unwrap();
}

fn chunk(stream: &mut TcpStream, text: &str) {
    write!(stream, "{:x}\r\n{text}\r\n", text.len()).unwrap();
    stream.flush().unwrap();
}

fn replay(stream: &mut TcpStream, fixture: &str) {
    chunked_head(stream);
    for line in fixture.lines() {
        chunk(stream, &format!("{line}\n"));
    }
    stream.write_all(b"0\r\n\r\n").unwrap();
}

fn hold_open(stream: &mut TcpStream) {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut buf = [0u8; 64];
    while matches!(stream.read(&mut buf), Ok(n) if n > 0) {}
}

fn dead_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    url
}

fn id(name: &str) -> ModelId<Llm> {
    format!("ollama/{name}").parse().unwrap()
}

fn e4b() -> ModelId<Llm> {
    id("gemma4:e4b")
}

#[derive(Default)]
struct Pieces(Vec<String>);

impl TextSink for Pieces {
    fn text(&mut self, piece: &str) {
        self.0.push(piece.to_owned());
    }
}

#[derive(Debug, PartialEq)]
enum Progress {
    Step(String),
    Bytes(u64, Option<u64>),
}

#[derive(Default)]
struct Recorder(Vec<Progress>);

impl ProvisionSink for Recorder {
    fn step(&mut self, label: &str) {
        self.0.push(Progress::Step(label.to_owned()));
    }

    fn bytes(&mut self, done: u64, total: Option<u64>) {
        self.0.push(Progress::Bytes(done, total));
    }
}

fn request() -> CompletionRequest<'static> {
    CompletionRequest {
        system: "Summarize the meeting.",
        user: "Alice: ship Friday.",
        max_output_tokens: 512,
        temperature: 0.5,
        threads: 4,
    }
}

fn complete(
    engine: &dyn LanguageModel,
    cancel: &CancelToken,
) -> (Result<String, EngineError>, Vec<String>) {
    let mut sink = Pieces::default();
    let result = engine.complete(&request(), &mut sink, cancel);
    (result, sink.0)
}

fn open(ollama: &Ollama, model: &ModelId<Llm>) -> Arc<dyn LanguageModel> {
    match ollama.open(model, &CancelToken::new()) {
        Ok(engine) => engine,
        Err(e) => panic!("open failed: {e}"),
    }
}

fn tags_server() -> Server {
    serve(|req, s| match req.path.as_str() {
        "/api/tags" => send_json(s, 200, TAGS_WITH),
        "/api/chat" => replay(s, CHAT_STREAM),
        _ => send_json(s, 404, "{}"),
    })
}

#[test]
fn chat_request_carries_the_context_window_and_options() {
    let server = tags_server();
    let ollama = Ollama::new(&server.url());
    let engine = open(&ollama, &e4b());
    let (result, _) = complete(&*engine, &CancelToken::new());
    result.unwrap();

    let bodies = server.bodies("/api/chat");
    assert_eq!(bodies.len(), 1);
    assert_eq!(
        bodies[0],
        json!({
            "model": "gemma4:e4b",
            "messages": [
                {"role": "system", "content": "Summarize the meeting."},
                {"role": "user", "content": "Alice: ship Friday."},
            ],
            "stream": true,
            "think": false,
            "options": {
                "num_ctx": 32768,
                "temperature": 0.5,
                "num_predict": 512,
                "num_thread": 4,
            },
        })
    );
    assert!(
        server
            .requests()
            .contains(&("POST".to_owned(), "/api/chat".to_owned()))
    );
}

#[test]
fn chat_request_sends_the_threads_as_num_thread() {
    let server = tags_server();
    let engine = open(&Ollama::new(&server.url()), &e4b());
    for threads in [2, 4] {
        let request = CompletionRequest {
            threads,
            ..request()
        };
        engine
            .complete(&request, &mut Pieces::default(), &CancelToken::new())
            .unwrap();
    }
    let sent: Vec<_> = server
        .bodies("/api/chat")
        .iter()
        .map(|b| b["options"]["num_thread"].clone())
        .collect();
    assert_eq!(sent, [json!(2), json!(4)]);
}

#[test]
fn the_context_window_can_be_overridden_and_is_reported_in_caps() {
    let server = tags_server();
    let ollama = Ollama::new(&server.url()).with_num_ctx(4096);
    for entry in ollama.catalog() {
        assert_eq!(entry.caps.context_tokens, 4096, "{}", entry.id);
    }
    let engine = open(&ollama, &e4b());
    complete(&*engine, &CancelToken::new()).0.unwrap();
    assert_eq!(server.bodies("/api/chat")[0]["options"]["num_ctx"], 4096);
}

#[test]
fn streamed_pieces_reach_the_sink_and_join_into_the_result() {
    let server = tags_server();
    let engine = open(&Ollama::new(&server.url()), &e4b());
    let (result, pieces) = complete(&*engine, &CancelToken::new());
    assert_eq!(
        pieces,
        ["## Summary", "\n\nThe team agreed", " to ship on Friday."]
    );
    assert_eq!(
        result.unwrap(),
        "## Summary\n\nThe team agreed to ship on Friday."
    );
}

#[test]
fn an_error_mid_stream_keeps_earlier_pieces_and_fails_as_a_crash() {
    let server = serve(|req, s| match req.path.as_str() {
        "/api/tags" => send_json(s, 200, TAGS_WITH),
        _ => replay(s, CHAT_ERROR),
    });
    let engine = open(&Ollama::new(&server.url()), &e4b());
    let (result, pieces) = complete(&*engine, &CancelToken::new());
    assert_eq!(pieces, ["## Summary", "\n\nThe team"]);
    let error = result.unwrap_err();
    assert!(matches!(error, EngineError::Crashed(ref m) if m.contains("exit status 2")));
    assert_eq!(error.class(), FailureClass::Transient);
}

#[test]
fn a_stream_that_ends_before_done_is_transient() {
    let server = serve(|req, s| match req.path.as_str() {
        "/api/tags" => send_json(s, 200, TAGS_WITH),
        _ => replay(s, CHAT_STREAM.lines().next().unwrap()),
    });
    let engine = open(&Ollama::new(&server.url()), &e4b());
    let (result, pieces) = complete(&*engine, &CancelToken::new());
    assert_eq!(pieces.len(), 1);
    let error = result.unwrap_err();
    assert!(
        matches!(error, EngineError::ServerUnavailable(_)),
        "{error}"
    );
    assert_eq!(error.class(), FailureClass::Transient);
}

#[test]
fn a_model_removed_after_open_is_reported_as_missing() {
    let server = serve(|req, s| match req.path.as_str() {
        "/api/tags" => send_json(s, 200, TAGS_WITH),
        _ => send_json(s, 404, CHAT_NOT_FOUND),
    });
    let engine = open(&Ollama::new(&server.url()), &e4b());
    let error = complete(&*engine, &CancelToken::new()).0.unwrap_err();
    assert!(matches!(error, EngineError::ModelMissing(ref m) if m == "gemma4:e4b"));
    assert_eq!(error.class(), FailureClass::NeedsUser);
}

#[test]
fn http_errors_map_to_engine_errors_by_status() {
    let cases = [
        (
            400,
            r#"{"error":"invalid options"}"#,
            FailureClass::Permanent,
        ),
        (503, r#"{"error":"server busy"}"#, FailureClass::Transient),
        (
            500,
            r#"{"error":"something broke"}"#,
            FailureClass::Permanent,
        ),
    ];
    for (status, body, class) in cases {
        let server = serve(move |req, s| match req.path.as_str() {
            "/api/tags" => send_json(s, 200, TAGS_WITH),
            _ => send_json(s, status, body),
        });
        let engine = open(&Ollama::new(&server.url()), &e4b());
        let error = complete(&*engine, &CancelToken::new()).0.unwrap_err();
        assert_eq!(error.class(), class, "{status}: {error}");
    }
}

#[test]
fn a_dead_server_is_unavailable_when_completing() {
    let server = tags_server();
    let engine = open(&Ollama::new(&server.url()), &e4b());
    drop(server);
    thread::sleep(Duration::from_millis(50));
    let error = complete(&*engine, &CancelToken::new()).0.unwrap_err();
    assert!(
        matches!(error, EngineError::ServerUnavailable(_)),
        "{error}"
    );
}

#[test]
fn availability_covers_ready_missing_and_down() {
    let with = serve(|_, s| send_json(s, 200, TAGS_WITH));
    let without = serve(|_, s| send_json(s, 200, TAGS_WITHOUT));
    let down = dead_url();

    assert_eq!(
        Ollama::new(&with.url()).availability(&e4b()),
        Availability::Ready
    );
    assert_eq!(
        Ollama::new(&without.url()).availability(&e4b()),
        Availability::NeedsProvision {
            bytes: Some(9_461_576_343)
        }
    );
    assert_eq!(
        Ollama::new(&down).availability(&e4b()),
        Availability::Unavailable(Unavailable {
            reason: UnavailableReason::ServerUnreachable { endpoint: down },
            fix: Some("Ollama is not running".to_owned()),
        })
    );
}

#[test]
fn availability_matches_names_the_way_ollama_does() {
    let server = serve(|_, s| send_json(s, 200, TAGS_WITH));
    let ollama = Ollama::new(&server.url());
    assert_eq!(ollama.availability(&id("llama3")), Availability::Ready);
    assert_eq!(
        ollama.availability(&id("llama3:latest")),
        Availability::Ready
    );
    assert_eq!(ollama.availability(&id("GEMMA4:E4B")), Availability::Ready);
    assert_eq!(
        ollama.availability(&id("gemma4:latest")),
        Availability::NeedsProvision { bytes: None }
    );
    assert_eq!(
        ollama.availability(&id("gemma4")),
        Availability::NeedsProvision { bytes: None }
    );
}

#[test]
fn availability_of_a_catalog_row_reports_its_known_size() {
    let server = serve(|_, s| send_json(s, 200, TAGS_WITHOUT));
    let ollama = Ollama::new(&server.url());
    let row = ollama
        .catalog()
        .into_iter()
        .find(|e| e.id == id("gemma4:12b"))
        .unwrap();
    assert_eq!(
        ollama.availability(&row.id),
        Availability::NeedsProvision {
            bytes: row.disk_bytes
        }
    );
}

#[test]
fn a_server_that_is_not_ollama_counts_as_unreachable() {
    let server = serve(|_, s| send_json(s, 200, "<html>nope</html>"));
    assert!(matches!(
        Ollama::new(&server.url()).availability(&e4b()),
        Availability::Unavailable(Unavailable {
            reason: UnavailableReason::ServerUnreachable { .. },
            ..
        })
    ));
}

#[test]
fn availability_gives_up_on_a_silent_server_quickly() {
    let server = serve(|_, s| hold_open(s));
    let started = Instant::now();
    let availability = Ollama::new(&server.url()).availability(&e4b());
    assert!(matches!(availability, Availability::Unavailable(_)));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn catalog_lists_the_two_gemma_rows_with_honest_explainers() {
    let rows = Ollama::new("http://127.0.0.1:11434").catalog();
    let ids: Vec<String> = rows.iter().map(|r| r.id.to_string()).collect();
    assert_eq!(ids, ["ollama/gemma4:e4b", "ollama/gemma4:12b"]);
    assert_eq!(
        rows.iter()
            .filter(|r| r.recommended)
            .map(|r| r.id.name())
            .collect::<Vec<_>>(),
        ["gemma4:e4b"]
    );
    for row in &rows {
        assert_eq!(
            row.caps,
            LlmCaps {
                context_tokens: 32_768,
                languages: dory_core::domain::LangSet::Any
            }
        );
        assert!(!row.about.summary.is_empty());
        assert!(!row.about.pros.is_empty() && !row.about.cons.is_empty());
    }
    let ollama = Ollama::new("http://127.0.0.1:11434");
    assert_eq!(ollama.id().as_str(), "ollama");
}

#[test]
fn open_checks_the_model_is_present() {
    let with = serve(|_, s| send_json(s, 200, TAGS_WITH));
    let without = serve(|_, s| send_json(s, 200, TAGS_WITHOUT));
    let down = dead_url();
    let token = CancelToken::new();

    assert!(Ollama::new(&with.url()).open(&e4b(), &token).is_ok());

    let missing = Ollama::new(&without.url())
        .open(&e4b(), &token)
        .err()
        .unwrap();
    assert!(matches!(missing, EngineError::ModelMissing(ref m) if m == "gemma4:e4b"));
    assert_eq!(missing.class(), FailureClass::NeedsUser);

    let unreachable = Ollama::new(&down).open(&e4b(), &token).err().unwrap();
    assert!(matches!(unreachable, EngineError::ServerUnavailable(_)));
    assert_eq!(unreachable.class(), FailureClass::Transient);
}

#[test]
fn open_never_pins_the_model_in_memory() {
    let server = tags_server();
    let engine = open(&Ollama::new(&server.url()), &e4b());
    complete(&*engine, &CancelToken::new()).0.unwrap();
    drop(engine);
    for body in server.bodies("/api/chat") {
        assert!(body.get("keep_alive").is_none(), "{body}");
    }
}

#[test]
fn count_tokens_never_underestimates_ordinary_text() {
    let server = tags_server();
    let engine = open(&Ollama::new(&server.url()), &e4b());
    assert_eq!(engine.count_tokens(""), 0);
    let english =
        "The quarterly roadmap review covered hiring, budget, and launch dates. ".repeat(50);
    assert!(engine.count_tokens(&english) as usize >= english.chars().count() / 4);
    let cjk = "今日の会議では予算について話しました。".repeat(20);
    assert!(engine.count_tokens(&cjk) as usize >= cjk.chars().count());
}

#[test]
fn pull_maps_status_and_aggregated_bytes_to_the_sink() {
    let server = serve(|_, s| replay(s, PULL_STREAM));
    let mut sink = Recorder::default();
    Ollama::new(&server.url())
        .provision(&e4b(), &mut sink, &CancelToken::new())
        .unwrap();

    assert_eq!(
        sink.0,
        [
            Progress::Step("Fetching manifest".into()),
            Progress::Step("Downloading".into()),
            Progress::Bytes(0, Some(6_600_000_000)),
            Progress::Bytes(1_650_000_000, Some(6_600_000_000)),
            Progress::Bytes(6_600_000_000, Some(6_600_000_000)),
            Progress::Bytes(6_600_001_480, Some(6_600_001_480)),
            Progress::Step("Verifying".into()),
            Progress::Step("Finishing".into()),
        ]
    );
    assert_eq!(
        server.bodies("/api/pull"),
        [json!({"model": "gemma4:e4b", "stream": true})]
    );
}

#[test]
fn pull_is_idempotent() {
    let server = serve(|_, s| replay(s, PULL_STREAM));
    let ollama = Ollama::new(&server.url());
    for _ in 0..2 {
        ollama
            .provision(&e4b(), &mut Recorder::default(), &CancelToken::new())
            .unwrap();
    }
    assert_eq!(server.bodies("/api/pull").len(), 2);
}

#[test]
fn pull_errors_map_to_provision_errors() {
    let missing = serve(|_, s| replay(s, PULL_NOT_FOUND));
    let error = Ollama::new(&missing.url())
        .provision(
            &id("nope:1b"),
            &mut Recorder::default(),
            &CancelToken::new(),
        )
        .unwrap_err();
    assert!(
        matches!(error, ProvisionError::NotProvisionable(_)),
        "{error}"
    );
    assert_eq!(error.class(), FailureClass::NeedsUser);

    let down = Ollama::new(&dead_url())
        .provision(&e4b(), &mut Recorder::default(), &CancelToken::new())
        .unwrap_err();
    assert!(matches!(down, ProvisionError::NotProvisionable(ref m) if m.contains("not running")));

    let truncated = serve(|_, s| replay(s, PULL_STALLED_HEAD));
    let error = Ollama::new(&truncated.url())
        .provision(&e4b(), &mut Recorder::default(), &CancelToken::new())
        .unwrap_err();
    assert!(matches!(error, ProvisionError::Network(_)), "{error}");
    assert_eq!(error.class(), FailureClass::Transient);
}

#[test]
fn pull_reports_a_full_disk() {
    let server = serve(|_, s| {
        replay(
            s,
            "{\"status\":\"pulling manifest\"}\n{\"error\":\"write /root/.ollama/models/blobs/sha256-1: no space left on device\"}",
        )
    });
    let error = Ollama::new(&server.url())
        .provision(&e4b(), &mut Recorder::default(), &CancelToken::new())
        .unwrap_err();
    assert!(matches!(error, ProvisionError::DiskFull { .. }), "{error}");
}

fn assert_cancels_within_a_second<T: Send + 'static>(
    ready: mpsc::Receiver<()>,
    run: impl FnOnce(CancelToken) -> T + Send + 'static,
) -> T {
    let token = CancelToken::new();
    let (done_tx, done_rx) = mpsc::channel();
    {
        let token = token.clone();
        thread::spawn(move || {
            let _ = done_tx.send(run(token));
        });
    }
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    thread::sleep(Duration::from_millis(100));
    assert!(
        done_rx.try_recv().is_err(),
        "the call returned before the cancel"
    );
    let started = Instant::now();
    token.cancel();
    let result = done_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("the call returned within 1 s of the cancel");
    assert!(started.elapsed() < Duration::from_secs(1));
    result
}

fn stalling_server(first_chunk: &'static str, route: &'static str) -> (Server, mpsc::Receiver<()>) {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let server = serve(move |req, s| {
        if req.path == "/api/tags" {
            return send_json(s, 200, TAGS_WITH);
        }
        assert_eq!(req.path, route);
        if first_chunk.is_empty() {
            tx.lock().unwrap().send(()).unwrap();
        } else {
            chunked_head(s);
            for line in first_chunk.lines() {
                chunk(s, &format!("{line}\n"));
            }
            tx.lock().unwrap().send(()).unwrap();
        }
        hold_open(s);
    });
    (server, rx)
}

#[test]
fn cancel_interrupts_a_chat_stalled_mid_stream() {
    let (server, ready) = stalling_server(CHAT_STREAM.lines().next().unwrap(), "/api/chat");
    let engine = open(&Ollama::new(&server.url()), &e4b());
    let (result, pieces) =
        assert_cancels_within_a_second(ready, move |token| complete(&*engine, &token));
    assert_eq!(pieces, ["## Summary"]);
    assert!(matches!(result, Err(EngineError::Cancelled)), "{result:?}");
}

#[test]
fn cancel_interrupts_a_chat_still_waiting_for_the_first_byte() {
    let (server, ready) = stalling_server("", "/api/chat");
    let engine = open(&Ollama::new(&server.url()), &e4b());
    let (result, pieces) =
        assert_cancels_within_a_second(ready, move |token| complete(&*engine, &token));
    assert!(pieces.is_empty());
    assert!(matches!(result, Err(EngineError::Cancelled)), "{result:?}");
    assert_eq!(result.unwrap_err().class(), FailureClass::Cancelled);
}

#[test]
fn cancel_interrupts_a_stalled_pull() {
    let (server, ready) = stalling_server(PULL_STALLED_HEAD, "/api/pull");
    let ollama = Ollama::new(&server.url());
    let result = assert_cancels_within_a_second(ready, move |token| {
        ollama.provision(&e4b(), &mut Recorder::default(), &token)
    });
    assert!(
        matches!(result, Err(ProvisionError::Cancelled)),
        "{result:?}"
    );
}

#[test]
fn a_token_cancelled_before_the_call_returns_at_once_without_a_request() {
    let server = tags_server();
    let ollama = Ollama::new(&server.url());
    let engine = open(&ollama, &e4b());
    let token = CancelToken::new();
    token.cancel();
    let before = server.requests().len();

    let started = Instant::now();
    assert!(matches!(
        complete(&*engine, &token).0,
        Err(EngineError::Cancelled)
    ));
    assert!(matches!(
        ollama.provision(&e4b(), &mut Recorder::default(), &token),
        Err(ProvisionError::Cancelled)
    ));
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(server.requests().len(), before);
}

#[test]
#[ignore = "needs a running Ollama with gemma4:e4b pulled"]
fn live_gemma_summarizes_a_short_transcript() {
    let url = std::env::var("OLLAMA_HOST")
        .map(|h| {
            if h.contains("://") {
                h
            } else {
                format!("http://{h}")
            }
        })
        .unwrap_or_else(|_| "http://127.0.0.1:11434".to_owned());
    let ollama = Ollama::new(&url);
    assert_eq!(ollama.availability(&e4b()), Availability::Ready);
    let engine = open(&ollama, &e4b());
    let mut sink = Pieces::default();
    let text = engine
        .complete(
            &CompletionRequest {
                system: "Summarize the meeting in one sentence.",
                user: "Alice: We ship on Friday.\nBob: Agreed, I will cut the release branch Thursday.",
                max_output_tokens: 200,
                temperature: 0.2,
                threads: 2,
            },
            &mut sink,
            &CancelToken::new(),
        )
        .unwrap();
    println!("pieces: {}\ntext: {text}", sink.0.len());
    assert!(!text.trim().is_empty());
    assert_eq!(sink.0.concat(), text);
}

#[test]
#[ignore = "needs a running Ollama"]
fn live_server_answers_availability() {
    let ollama = Ollama::new("http://127.0.0.1:11434");
    let availability = ollama.availability(&e4b());
    println!("availability: {availability:?}");
    assert!(!matches!(availability, Availability::Unavailable(_)));
}
