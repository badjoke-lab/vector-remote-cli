//! Vector Remote CLI: one owner, one persistent local shell, encrypted DM I/O.
//! Supported targets: macOS / Linux. This is an interactive terminal transported
//! over messages, not a real-time terminal emulator.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::{
    collections::{HashSet, VecDeque},
    env,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc as std_mpsc, Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;
use vector_sdk::{nostr::{FromBech32, PublicKey, ToBech32}, BotEvent, VectorBot};

const MAX_COMMAND_BYTES: usize = 2048;
const MAX_MESSAGE_AGE_MS: u64 = 90_000;
const MAX_FUTURE_SKEW_MS: u64 = 30_000;
const MAX_OUTPUT_MESSAGE_CHARS: usize = 1550;
const MAX_OUTPUT_MESSAGES_PER_MINUTE: usize = 80;
const PROCESSED_ID_LIMIT: usize = 20_000;

// Run a persistent native shell over PTY stdin without interactive line editing.
// Interactive zsh uses ZLE and redisplays commands even with "stty -echo".
// "zsh -f -s" reads commands incrementally, preserves cd/env and has no prompt.
// The startup handshake gates DM commands until PTY echo is disabled.
const SHELL_READY_MARKER: &str = "__VECTOR_CLI_READY_20261021__";
const SHELL_INIT: &str = "stty -echo\nprintf '%s%s\\n' '__VECTOR_CLI_' 'READY_20261021__'\n";

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn is_fresh(at_ms: u64, current_ms: u64) -> bool {
    at_ms > 0
        && at_ms >= current_ms.saturating_sub(MAX_MESSAGE_AGE_MS)
        && at_ms <= current_ms.saturating_add(MAX_FUTURE_SKEW_MS)
}

fn canonical_npub(value: &str) -> Result<String, String> {
    // Accept the human npub encoding, not an arbitrary profile nickname.
    if !value.starts_with("npub1") {
        return Err("VECTOR_OWNER_NPUB must start with npub1".into());
    }
    let pk = PublicKey::from_bech32(value).map_err(|e| format!("invalid owner npub: {e}"))?;
    pk.to_bech32().map_err(|e| format!("unable to normalize owner npub: {e}"))
}

fn prepare_private_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| format!("unable to create {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("unable to protect data directory: {e}"))?;
    }
    Ok(())
}

// A disk-backed, bounded replay cache. Only event IDs are persisted; no commands or output.
struct SeenEvents {
    path: PathBuf,
    ordered: VecDeque<String>,
    set: HashSet<String>,
}
impl SeenEvents {
    fn open(path: PathBuf) -> Result<Self, String> {
        let contents = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("cannot read replay cache: {e}")),
        };
        let lines: Vec<String> = contents.lines().rev().take(PROCESSED_ID_LIMIT)
            .map(str::to_string).collect();
        let mut ordered = VecDeque::new();
        let mut set = HashSet::new();
        for id in lines.into_iter().rev() {
            if set.insert(id.clone()) { ordered.push_back(id); }
        }
        let cache = Self { path, ordered, set };
        cache.rewrite()?;
        Ok(cache)
    }
    fn rewrite(&self) -> Result<(), String> {
        let tmp = self.path.with_extension("tmp");
        let mut file = OpenOptions::new().write(true).create(true).truncate(true).open(&tmp)
            .map_err(|e| format!("cannot open replay cache: {e}"))?;
        for id in &self.ordered {
            writeln!(file, "{id}").map_err(|e| format!("cannot write replay cache: {e}"))?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("cannot protect replay cache: {e}"))?;
        }
        file.sync_all().map_err(|e| format!("cannot sync replay cache: {e}"))?;
        fs::rename(&tmp, &self.path).map_err(|e| format!("cannot update replay cache: {e}"))?;
        Ok(())
    }
    fn claim(&mut self, id: &str) -> Result<bool, String> {
        if id.is_empty() || id.contains('\n') || id.contains('\r') {
            return Err("invalid event identifier".into());
        }
        if self.set.contains(id) { return Ok(false); }
        self.set.insert(id.to_string());
        self.ordered.push_back(id.to_string());
        if self.ordered.len() > PROCESSED_ID_LIMIT {
            if let Some(old) = self.ordered.pop_front() { self.set.remove(&old); }
        }
        self.rewrite()?;
        Ok(true)
    }
}

#[derive(Clone)]
struct ShellSession {
    tx: std_mpsc::Sender<Control>,
    generation: u64,
}
enum Control { WriteLine(String), Interrupt, Terminate }
enum TerminalEvent { Data(Vec<u8>), Exited }

struct Shared {
    owner_npub: String,
    seen: Mutex<SeenEvents>,
    session: Arc<Mutex<Option<ShellSession>>>,
    generation: Arc<AtomicU64>,
}

fn shell_path() -> Result<String, String> {
    let shell = env::var("VECTOR_SHELL")
        .or_else(|_| env::var("SHELL"))
        .unwrap_or_else(|_| "/bin/sh".into());
    if !Path::new(&shell).is_absolute() || !Path::new(&shell).is_file() {
        return Err(format!("shell executable does not exist: {shell}"));
    }
    Ok(shell)
}

fn create_terminal(
    bot: VectorBot,
    owner: String,
    generation: Arc<AtomicU64>,
    sessions: Arc<Mutex<Option<ShellSession>>>,
    session_id: u64,
) -> Result<ShellSession, String> {
    let shell = shell_path()?;
    let pair = native_pty_system()
        .openpty(PtySize { rows: 30, cols: 100, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| format!("openpty failed: {e}"))?;
    let mut cmd = CommandBuilder::new(&shell);
    // Noninteractive script mode keeps the shell process alive but disables
    // zsh's ZLE/prompt and supports sequential command lines on its PTY stdin.
    if shell.ends_with("/zsh") { cmd.arg("-f"); }
    cmd.arg("-s");
    cmd.env("PS1", "");
    cmd.env("PROMPT", "");
    cmd.env("RPROMPT", "");
    cmd.env("PROMPT_EOL_MARK", "");
    cmd.env("TERM", "dumb");
    cmd.env("PAGER", "cat");
    cmd.env("GIT_PAGER", "cat");
    let mut child = pair.slave.spawn_command(cmd)
        .map_err(|e| format!("shell launch failed: {e}"))?;
    let mut reader = pair.master.try_clone_reader()
        .map_err(|e| format!("cannot read PTY: {e}"))?;
    let mut writer = pair.master.take_writer()
        .map_err(|e| format!("cannot write PTY: {e}"))?;
    drop(pair.slave);

    // Bootstrap is written before spawning the command writer. Commands
    // received immediately after "connect" remain queued in order.
    writer.write_all(SHELL_INIT.as_bytes())
        .and_then(|_| writer.flush())
        .map_err(|e| format!("shell initialization failed: {e}"))?;
    let (control_tx, control_rx) = std_mpsc::channel::<Control>();
    let (ready_tx, ready_rx) = std_mpsc::sync_channel::<bool>(1);
    let (out_tx, out_rx) = mpsc::channel::<TerminalEvent>(32);
    let end_tx = out_tx.clone();
    std::thread::spawn(move || {
        // Don't write commands into the PTY until echo and ZLE have been
        // disabled. This also prevents startup writes being interpreted as
        // user input before the interactive shell is accepting commands.
        if ready_rx.recv_timeout(Duration::from_secs(15)).ok() == Some(true) {
            while let Ok(control) = control_rx.recv() {
                let ok = match control {
                Control::WriteLine(line) => {
                    let result = writer.write_all(line.as_bytes())
                        .and_then(|_| writer.write_all(b"\n"))
                        .and_then(|_| writer.flush());
                    eprintln!("[PTY WRITE] command bytes={} ok={}", line.len(), result.is_ok());
                    result
                },
                Control::Interrupt => writer.write_all(b"\x03").and_then(|_| writer.flush()),
                Control::Terminate => {
                    let _ = writer.write_all(b"\x03exit\n");
                    let _ = writer.flush();
                    break;
                }
            };
                if ok.is_err() { break; }
            }
        } else {
            eprintln!("[PTY ERROR] shell startup handshake failed or timed out");
        }
        let _ = child.kill();
        let _ = child.wait();
    });
    std::thread::spawn(move || {
        let mut bytes = [0_u8; 4096];
        let mut startup = Vec::<u8>::new();
        let mut ready = false;
        loop {
            match reader.read(&mut bytes) {
                Ok(0) => { eprintln!("[PTY EXIT] EOF"); break; }
                Err(e) => { eprintln!("[PTY EXIT] read error: {e}"); break; }
                Ok(n) => {
                    let data = if !ready {
                        startup.extend_from_slice(&bytes[..n]);
                        if let Some(at) = startup.windows(SHELL_READY_MARKER.len())
                            .position(|x| x == SHELL_READY_MARKER.as_bytes()) {
                            let mut tail = startup.split_off(at + SHELL_READY_MARKER.len());
                            if tail.starts_with(b"\r\n") { tail.drain(..2); }
                            else if tail.starts_with(b"\n") { tail.remove(0); }
                            startup.clear();
                            ready = true;
                            eprintln!("[PTY READY] shell initialization completed");
                            let _ = ready_tx.send(true);
                            tail
                        } else {
                            if startup.len() > 8192 {
                                eprintln!("[PTY ERROR] startup exceeded 8KiB without ready marker");
                                break;
                            }
                            continue;
                        }
                    } else { bytes[..n].to_vec() };
                    if !data.is_empty() {
                        eprintln!("[PTY OUT] bytes={}", data.len());
                        if out_tx.blocking_send(TerminalEvent::Data(data)).is_err() { break; }
                    }
                }
            }
        }
        if !ready {
            eprintln!("[PTY ERROR] shell never became ready");
            let _ = ready_tx.send(false);
        }
        let _ = end_tx.blocking_send(TerminalEvent::Exited);
    });
    tokio::spawn(pump_output(bot, owner, generation, sessions, session_id, out_rx));
    Ok(ShellSession { tx: control_tx, generation: session_id })
}

// ANSI/terminal-control filtering: remove CSI, OSC, and stray control codes.
fn clean_terminal_output(input: &[u8]) -> String {
    let s = String::from_utf8_lossy(input);
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(ch) = it.next() {
        if ch == '\x1b' {
            match it.next() {
                Some('[') => {
                    // CSI terminates at a final byte in the ASCII 0x40..=0x7E range.
                    for c in it.by_ref() { if ('@'..='~').contains(&c) { break; } }
                }
                Some(']') => {
                    // OSC ends in BEL or ESC followed by backslash.
                    while let Some(c) = it.next() {
                        if c == '\x07' { break; }
                        if c == '\x1b' && it.peek() == Some(&'\\') { it.next(); break; }
                    }
                }
                _ => {}
            }
        } else if ch == '\r' {
            // PTYs commonly use CRLF. Render that as one newline, not two.
            if it.peek() == Some(&'\n') { it.next(); }
            out.push('\n');
        }
        else if ch == '\n' || ch == '\t' || !ch.is_control() { out.push(ch); }
    }
    out
}

// Limit per-message size without splitting a UTF-8 codepoint.
fn take_chars(text: &str, max_chars: usize) -> (&str, &str) {
    let at = text.char_indices().nth(max_chars).map(|(i, _)| i).unwrap_or(text.len());
    text.split_at(at)
}

async fn pump_output(
    bot: VectorBot,
    owner: String,
    generation: Arc<AtomicU64>,
    sessions: Arc<Mutex<Option<ShellSession>>>,
    id: u64,
    mut rx: mpsc::Receiver<TerminalEvent>,
) {
    let mut window = Instant::now();
    let mut sends_in_window = 0usize;
    let mut notified_limit = false;
    while let Some(event) = rx.recv().await {
        if generation.load(Ordering::SeqCst) != id { break; }
        let mut raw = Vec::new();
        let mut exited = match event { TerminalEvent::Data(data) => { raw.extend(data); false }, TerminalEvent::Exited => true };
        // Batch small writes to reduce relay traffic and notify about truncation once.
        let until = Instant::now() + Duration::from_millis(250);
        while !exited && raw.len() < 5000 {
            let remaining = until.saturating_duration_since(Instant::now());
            if remaining.is_zero() { break; }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Some(TerminalEvent::Data(data))) => raw.extend(data),
                Ok(Some(TerminalEvent::Exited)) => { exited = true; break; },
                _ => break,
            }
        }
        if window.elapsed() >= Duration::from_secs(60) {
            window = Instant::now(); sends_in_window = 0; notified_limit = false;
        }
        let cleaned = clean_terminal_output(&raw);
        let mut text = cleaned.trim_matches('\n').to_string();
        // Never emit empty "[CLI]" bubbles for startup/line-ending noise.
        if text.trim().is_empty() && !exited { continue; }
        while !text.is_empty() {
            if sends_in_window >= MAX_OUTPUT_MESSAGES_PER_MINUTE {
                if !notified_limit {
                    let _ = bot.dm(&owner).send("[CLI] 出力が多すぎるため、残りの出力を一時的に省略しました。シェルの実行は継続中です。").await;
                    notified_limit = true;
                }
                break;
            }
            let (piece, rest) = take_chars(&text, MAX_OUTPUT_MESSAGE_CHARS);
            if let Err(e) = bot.dm(&owner).send(&format!("[CLI]\n{piece}")).await {
                eprintln!("[TX ERROR] shell output could not be sent: {e}");
            }
            sends_in_window += 1;
            text = rest.to_string();
        }
        if exited {
            if generation.load(Ordering::SeqCst) == id {
                if let Ok(mut guard) = sessions.lock() {
                    if guard.as_ref().is_some_and(|s| s.generation == id) {
                        *guard = None;
                    }
                }
                generation.fetch_add(1, Ordering::SeqCst);
                let _ = bot.dm(&owner).send("[CLI] シェルが終了しました。/connect で再接続できます。").await;
            }
            break;
        }
    }
}

async fn reply(bot: &VectorBot, owner: &str, text: &str) {
    match bot.dm(owner).send(text).await {
        Ok(_) => eprintln!("[TX] DM accepted for sending ({} bytes)", text.len()),
        Err(e) => eprintln!("[TX ERROR] DM send failed: {e}"),
    }
}

async fn handle(bot: VectorBot, msg: vector_sdk::IncomingMessage, shared: Arc<Shared>) {
    // Keep diagnostics content-free: never log commands, output, or secret keys.
    eprintln!("[RX] message id={} dm={} file={} own={} from_chat={} from_payload={} age_ms={}",
        msg.message.id, !msg.is_group, msg.is_file, msg.is_mine(),
        msg.chat_id, msg.message.npub.as_deref().unwrap_or("<none>"),
        now_ms() as i128 - msg.message.at as i128);
    if msg.is_mine() || msg.is_group || msg.is_file {
        eprintln!("[RX SKIP] own/group/file message");
        return;
    }
    if msg.chat_id != shared.owner_npub
        || msg.message.npub.as_deref().is_some_and(|n| n != shared.owner_npub.as_str()) {
        eprintln!("[RX SKIP] unauthorized sender");
        return;
    }
    if !is_fresh(msg.message.at, now_ms()) {
        eprintln!("[RX SKIP] stale/future message timestamp");
        return;
    }
    {
        let Ok(mut seen) = shared.seen.lock() else { return; };
        match seen.claim(&msg.message.id) {
            Ok(true) => {}
            Ok(false) => return,
            Err(e) => { eprintln!("failed to record message ID: {e}"); return; }
        }
    }
    let input = msg.text().trim();
    if input.is_empty() { return; }
    eprintln!("[RX OK] accepted command ({} bytes)", input.len());
    let command = match input {
        "/help" | "help" => {
            reply(&bot, &shared.owner_npub, "Vector Remote CLI\n/connect — シェルを開始\n/close — シェルを終了\n/interrupt — Ctrl+C\n/status — 状態確認\nそれ以外のメッセージはシェルへの1行入力です。\n※ Vim等の全画面アプリはチャットUIには向きません。").await;
            return;
        }
        "/status" | "status" => {
            let online = shared.session.lock().map(|s| s.is_some()).unwrap_or(false);
            reply(&bot, &shared.owner_npub, if online { "[CLI] セッション接続中" } else { "[CLI] 未接続。/connect を送信してください。" }).await;
            return;
        }
        "/connect" | "connect" => {
            let result = {
                let mut guard = match shared.session.lock() { Ok(g) => g, Err(_) => return };
                if guard.is_some() { Ok(false) } else {
                    let id = shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
                    match create_terminal(bot.clone(), shared.owner_npub.clone(), shared.generation.clone(), Arc::clone(&shared.session), id) {
                        Ok(session) => { *guard = Some(session); Ok(true) }
                        Err(e) => Err(e),
                    }
                }
            };
            match result {
                Ok(true) => reply(&bot, &shared.owner_npub, "[CLI] 接続しました。コマンドをそのままDMで送信してください。").await,
                Ok(false) => reply(&bot, &shared.owner_npub, "[CLI] すでに接続中です。/status または /close を使用してください。").await,
                Err(e) => reply(&bot, &shared.owner_npub, &format!("[CLI] 接続失敗: {e}")).await,
            }
            return;
        }
        "/close" | "close" => {
            let session = shared.session.lock().ok().and_then(|mut s| s.take());
            if let Some(session) = session {
                shared.generation.fetch_add(1, Ordering::SeqCst);
                let _ = session.tx.send(Control::Terminate);
                reply(&bot, &shared.owner_npub, "[CLI] シェルを終了しました。").await;
            } else { reply(&bot, &shared.owner_npub, "[CLI] すでに未接続です。").await; }
            return;
        }
        "/interrupt" | "interrupt" => Control::Interrupt,
        _ => {
            if input.len() > MAX_COMMAND_BYTES || input.contains('\n') || input.contains('\r') {
                reply(&bot, &shared.owner_npub, "[CLI] コマンドは改行なし・最大2048バイトです。").await;
                return;
            }
            Control::WriteLine(input.to_string())
        }
    };
    let result = shared.session.lock().ok().and_then(|s| s.as_ref().map(|x| (x.tx.clone(),x.generation)));
    match result {
        Some((tx, id)) if shared.generation.load(Ordering::SeqCst) == id => {
            if tx.send(command).is_err() {
                reply(&bot, &shared.owner_npub, "[CLI] シェルへの送信に失敗しました。/close → /connect を試してください。").await;
            }
        }
        _ => reply(&bot, &shared.owner_npub, "[CLI] 未接続です。最初に /connect を送信してください。").await,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let owner_npub = canonical_npub(&env::var("VECTOR_OWNER_NPUB")
        .map_err(|_| "VECTOR_OWNER_NPUB (your npub1...) is required")?)?;
    let data_dir = env::var("VECTOR_DATA_DIR").map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./vector-remote-cli-data"));
    prepare_private_dir(&data_dir)?;
    let mut builder = VectorBot::builder().data_dir(&data_dir);
    if let Ok(nsec) = env::var("VECTOR_NSEC") { builder = builder.nsec(nsec); }
    #[cfg(feature = "tor")]
    if env::var("VECTOR_USE_TOR").as_deref() == Ok("1") { builder = builder.tor(); }
    let bot = builder.build().await?;
    println!("Vector Remote CLI running as {}. Owner: {}", bot.npub(), owner_npub);
    println!("Only encrypted DMs from the configured owner are accepted.");
    let state = Arc::new(Shared {
        owner_npub,
        seen: Mutex::new(SeenEvents::open(data_dir.join("seen-events.log"))?),
        session: Arc::new(Mutex::new(None)),
        generation: Arc::new(AtomicU64::new(0)),
    });
    eprintln!("[START] Connecting to Vector relays. Wait for [READY] before testing DMs.");
    bot.on_event(move |bot, event| {
        let state = Arc::clone(&state);
        async move {
            match event {
                BotEvent::Ready { communities } => {
                    eprintln!("[READY] Vector realtime subscription registered (communities={communities})");
                    // Outbound proof only: never opens a shell automatically.
                    reply(&bot, &state.owner_npub,
                        "[CLI] Bot connected. Reply with connect (no slash) to open a shell.").await;
                }
                BotEvent::Message(msg) => handle(bot, msg, state).await,
                _ => {}
            }
        }
    }).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn freshness_rejects_replay_and_future() {
        let now = 10_000_000;
        assert!(is_fresh(now, now));
        assert!(!is_fresh(now - MAX_MESSAGE_AGE_MS - 1, now));
        assert!(!is_fresh(now + MAX_FUTURE_SKEW_MS + 1, now));
        assert!(!is_fresh(0, now));
    }
    #[test]
    fn terminal_output_removes_ansi() {
        assert_eq!(clean_terminal_output(b"abc\x1b[31mred\x1b[0m\r\n"), "abcred\n");
        // Do not strip actual program output just because it looks like a shell prompt.
        assert_eq!(clean_terminal_output(b"100%\r\n"), "100%\n");
    }
    #[test]
    fn chunk_on_unicode_boundary() {
        let (a,b) = take_chars("日本語", 2);
        assert_eq!((a,b), ("日本", "語"));
    }
    #[test]
    fn quiet_shell_has_no_prompt_or_command_echo_and_keeps_cwd() {
        // Exercise the production startup protocol with a permanently open
        // PTY writer, not the obsolete read/eval driver.
        let pair = native_pty_system().openpty(PtySize {
            rows: 20, cols: 80, pixel_width: 0, pixel_height: 0,
        }).expect("open PTY");
        let shell = if cfg!(target_os = "macos") { "/bin/zsh" } else { "/bin/sh" };
        let mut cmd = CommandBuilder::new(shell);
        if shell.ends_with("/zsh") { cmd.arg("-f"); }
        cmd.arg("-s");
        cmd.env("PS1", "");
        cmd.env("PROMPT", "");
        cmd.env("RPROMPT", "");
        cmd.env("PROMPT_EOL_MARK", "");
        let mut child = pair.slave.spawn_command(cmd).expect("launch quiet shell");
        let mut reader = pair.master.try_clone_reader().expect("reader");
        let mut writer = pair.master.take_writer().expect("writer");
        drop(pair.slave);
        writer.write_all(SHELL_INIT.as_bytes()).expect("initialize PTY");
        writer.flush().expect("flush init");
        // Wait for the real ready marker before writing more input, as the
        // production PTY controller now does.
        let mut output = Vec::new();
        let mut bytes = [0u8; 2048];
        while !output.windows(SHELL_READY_MARKER.len())
            .any(|w| w == SHELL_READY_MARKER.as_bytes()) {
            let n = reader.read(&mut bytes).expect("read init");
            assert!(n > 0, "shell stopped before ready handshake");
            output.extend_from_slice(&bytes[..n]);
        }
        writer.write_all(b"pwd\ncd /tmp\npwd\nprintf '100%%\\n'\nexit\n").expect("send");
        writer.flush().expect("flush commands");
        drop(writer);
        drop(pair.master);
        reader.read_to_end(&mut output).expect("read results");
        child.wait().expect("shell exit");
        let output = clean_terminal_output(&output);
        let marker = output.find(SHELL_READY_MARKER).unwrap_or_else(|| panic!("shell never ready: {output:?}"));
        let results = &output[marker+SHELL_READY_MARKER.len()..];
        assert!(!results.contains("pwd"), "unexpected command echo: {results:?}");
        assert!(!results.contains("lyla@"), "unexpected prompt: {results:?}");
        assert!(results.contains("/tmp\n"), "cwd did not persist: {results:?}");
        assert!(results.contains("100%\n"), "legitimate percent output lost: {results:?}");
    }

    #[test]
    fn persistent_shell_streams_results_without_closing_writer() {
        // Regresses the macOS failure: RX handled "pwd" but no PTY data was
        // returned. Unlike the batch test, no writer EOF or shell exit is
        // involved before each response must arrive.
        let pair = native_pty_system().openpty(PtySize {
            rows: 20, cols: 80, pixel_width: 0, pixel_height: 0,
        }).expect("open PTY");
        let shell = if cfg!(target_os = "macos") { "/bin/zsh" } else { "/bin/sh" };
        let mut cmd = CommandBuilder::new(shell);
        if shell.ends_with("/zsh") { cmd.arg("-f"); }
        cmd.arg("-s");
        cmd.env("PS1", "");
        cmd.env("PROMPT", "");
        cmd.env("RPROMPT", "");
        cmd.env("PROMPT_EOL_MARK", "");
        let mut child = pair.slave.spawn_command(cmd).expect("launch interactive shell");
        let mut reader = pair.master.try_clone_reader().expect("reader");
        let mut writer = pair.master.take_writer().expect("writer");
        drop(pair.slave);
        let (tx, rx) = std_mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() { break; }
            }
        });

        writer.write_all(SHELL_INIT.as_bytes()).expect("init");
        writer.flush().expect("flush init");
        let mut observed = String::new();
        let receive_until = |observed: &mut String, marker: &str| {
            for _ in 0..60 {
                if observed.contains(marker) { return; }
                let buf = rx.recv_timeout(Duration::from_millis(200))
                    .expect("shell did not stream output before writer EOF");
                observed.push_str(&String::from_utf8_lossy(&buf));
            }
            panic!("output marker not seen: {marker}");
        };
        receive_until(&mut observed, SHELL_READY_MARKER);
        let at = observed.find(SHELL_READY_MARKER).expect("ready marker");
        observed = observed[at + SHELL_READY_MARKER.len()..].to_string();

        writer.write_all(b"printf 'vector-first-reply\\n'\n").expect("first command");
        writer.flush().expect("first flush");
        receive_until(&mut observed, "vector-first-reply");
        writer.write_all(b"cd /tmp\npwd\n").expect("cwd commands");
        writer.flush().expect("cwd flush");
        receive_until(&mut observed, "/tmp");
        assert!(!observed.contains("lyla@"), "interactive prompt leaked");
        writer.write_all(b"exit\n").expect("exit");
        writer.flush().expect("exit flush");
        child.wait().expect("wait for shell");
    }

    #[test]
    fn pty_can_start_local_shell() {
        let pair = native_pty_system().openpty(PtySize {
            rows: 20, cols: 80, pixel_width: 0, pixel_height: 0,
        }).expect("open PTY");
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.arg("-c");
        cmd.arg("printf vector-pty-ok");
        let mut child = pair.slave.spawn_command(cmd).expect("launch shell");
        let mut reader = pair.master.try_clone_reader().expect("reader");
        drop(pair.slave);
        drop(pair.master);
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).expect("read from PTY");
        child.wait().expect("wait for shell");
        assert!(String::from_utf8_lossy(&bytes).contains("vector-pty-ok"));
    }
}