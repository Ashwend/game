use std::{
    io::{BufRead, BufReader},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};

use crate::{
    save::{WorldSave, save_world_file},
    world::{MapType, ProceduralMapSize},
};

/// Default display names for the two spawned test clients. `player1` lands on
/// the left monitor and `player2` on the right (see [`test_client_layouts`] and
/// the client-side `reposition_test_window_system`); the names mirror that
/// left-to-right ordering so the window you're looking at is obvious.
const DEFAULT_NAMES: [&str; 2] = ["player1", "player2"];
/// Stable but distinct account IDs so the server treats each spawned client
/// as a separate player. Different from the default bypass ID
/// (`76561197960287930`) to avoid colliding with a real local-dev session.
const TEST_ACCOUNT_IDS: [u64; 2] = [76_561_197_960_287_001, 76_561_197_960_287_002];
/// Map size for the ephemeral test world, the smallest procedural map so
/// the helper boots fast and streams cheaply. Single source of truth: the
/// seed save is generated at this size *and* the spawned `server --map-size`
/// flag is derived from it, so the two can't drift apart and trip the
/// "save was generated as X but Y was requested" guard in `cli.rs`.
const TEST_MAP_SIZE: ProceduralMapSize = ProceduralMapSize::Small;
/// How long we wait for the server to advertise its listening port before
/// giving up. The server prints `Lightyear game server listening on …` once
/// it's ready, so on a warm rebuild this typically takes a few hundred ms.
const SERVER_READY_TIMEOUT: Duration = Duration::from_secs(45);

/// Pixel size of each test window. Compact enough that two fit side-by-side
/// on a 1920-wide display with comfortable margins, tall enough to fit the
/// inventory panel without scrolling. Actual on-screen placement is decided
/// by the client once it can query the real monitor size; see
/// `reposition_test_window_system`.
const TEST_WINDOW_WIDTH: u32 = 880;
const TEST_WINDOW_HEIGHT: u32 = 620;
/// Horizontal gap (px) between the two test windows.
const TEST_WINDOW_GAP: i32 = 24;
/// Distance in meters each test player is pushed away from the world
/// spawn point so the two characters face each other across a small gap
/// (≈ 2 × this value). Tuned so they're close enough to see each other's
/// nameplate/voice indicators and far enough that movement interpolation
/// is easy to read.
const TEST_PLAYER_OFFSET_X: f32 = 1.25;

/// Spawn a fresh local server with an ephemeral test world and two client
/// windows that auto-connect with distinct identities. Blocks until both
/// clients exit, then shuts down the server.
pub(super) fn run_multiplayer_test(port: u16, names_override: Option<Vec<String>>) -> Result<()> {
    let names = resolved_names(names_override);
    let port = resolve_port(port)?;
    let bind: SocketAddr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port);

    let exe = std::env::current_exe()
        .context("could not resolve the current executable path for multiplayer-test")?;
    let world_dir = tempdir(&format!("game-multiplayer-test-{}", std::process::id()))?;
    let world_save = world_dir.path.join("test.save");

    println!("multiplayer-test: starting server on {bind}");
    println!(
        "multiplayer-test: temporary world save → {}",
        world_save.display()
    );

    // Pre-create the temp save with the smallest procedural map so the
    // helper boots into a tiny world, quick to generate, cheap to stream,
    // and uses the same map path as a real player-created world.
    let mut seeded = WorldSave::new_with_map(
        "Multiplayer Test",
        None,
        MapType::Procedural {
            seed: 0,
            size: TEST_MAP_SIZE,
        },
    );
    // Flag both test clients as admins so they can drive `/test-kit` and
    // `/tp` straight out of the gate, those commands are admin-gated
    // and the multiplayer-test loop is the place where they're most
    // useful for verifying PvP / death / respawn.
    for account_id in TEST_ACCOUNT_IDS.map(crate::protocol::AccountId) {
        if !seeded.admins.contains(&account_id) {
            seeded.admins.push(account_id);
        }
    }
    save_world_file(&world_save, &seeded).context("could not seed multiplayer-test world save")?;

    let mut server = spawn_server(&exe, &world_save, bind)?;
    if let Err(error) = wait_for_server_ready(&mut server) {
        let _ = server
            .child
            .lock()
            .ok()
            .and_then(|mut child| child.kill().ok());
        bail!("server did not become ready: {error:#}");
    }

    println!("multiplayer-test: server ready, launching clients {names:?}");
    let layouts = test_client_layouts();
    let mut clients = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let account_id = TEST_ACCOUNT_IDS[index];
        let layout = layouts[index];
        let child = spawn_client(&exe, bind, name, account_id, layout)
            .with_context(|| format!("could not spawn test client {name}"))?;
        clients.push(child);
    }

    let exit_signal = Arc::new(AtomicBool::new(false));
    let signal_for_handler = exit_signal.clone();
    ctrlc_listener(signal_for_handler);

    wait_for_clients(&mut clients, exit_signal.clone());
    println!("multiplayer-test: clients exited, shutting down server");
    server.shutdown();

    let _ = world_dir;
    Ok(())
}

fn resolved_names(override_names: Option<Vec<String>>) -> [String; 2] {
    let mut names = [DEFAULT_NAMES[0].to_owned(), DEFAULT_NAMES[1].to_owned()];
    if let Some(custom) = override_names {
        for (slot, name) in names.iter_mut().zip(custom) {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                *slot = trimmed.to_owned();
            }
        }
    }
    names
}

fn resolve_port(requested: u16) -> Result<u16> {
    if requested != 0 {
        return Ok(requested);
    }
    // Bind+drop a TCP listener to reserve a port that's almost certainly
    // free for the UDP server seconds later. Not bulletproof, the kernel
    // can technically re-allocate it, but in practice it gives us a
    // distinct port per test run with no manual configuration.
    let listener = TcpListener::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))
        .context("could not pick a free port for multiplayer-test")?;
    let port = listener
        .local_addr()
        .context("could not read picked port")?
        .port();
    drop(listener);
    Ok(port)
}

struct ServerProcess {
    child: Arc<Mutex<Child>>,
    addr: SocketAddr,
    ready_rx: std::sync::mpsc::Receiver<ServerReady>,
}

enum ServerReady {
    Listening,
    Exited,
}

/// The `server --map-size <token>` value matching a [`ProceduralMapSize`].
/// Mirrors the clap `MapSizeArg` value-enum tokens in `cli.rs`; the
/// exhaustive match means a new map size forces this to be updated.
fn map_size_cli_token(size: ProceduralMapSize) -> &'static str {
    match size {
        ProceduralMapSize::Small => "small",
        ProceduralMapSize::Medium => "medium",
        ProceduralMapSize::Large => "large",
    }
}

fn spawn_server(
    exe: &std::path::Path,
    save: &std::path::Path,
    addr: SocketAddr,
) -> Result<ServerProcess> {
    let mut command = Command::new(exe);
    command
        .arg("server")
        .arg("--bind")
        .arg(addr.to_string())
        .arg("--world")
        .arg(save)
        // Localhost test server: bypass WorkOS and admit each spawned client by
        // the account id + name it claims via the environment.
        .arg("--auth")
        .arg("no-auth")
        // Match the size the seed save was generated at. Without this the
        // server falls back to its `--map-size` default and the size guard
        // rejects the (smaller) seed save.
        .arg("--map-size")
        .arg(map_size_cli_token(TEST_MAP_SIZE))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    let mut child = command
        .spawn()
        .with_context(|| format!("could not spawn server binary {}", exe.display()))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("server stdout pipe missing"))?;

    let (tx, ready_rx) = std::sync::mpsc::channel();
    thread::Builder::new()
        .name("multiplayer-test-server-stdout".to_owned())
        .spawn(move || {
            let reader = BufReader::new(stdout);
            let mut signalled = false;
            for line in reader.lines().map_while(Result::ok) {
                println!("[server] {line}");
                if !signalled && line.contains("listening on") {
                    let _ = tx.send(ServerReady::Listening);
                    signalled = true;
                }
            }
            if !signalled {
                let _ = tx.send(ServerReady::Exited);
            }
        })
        .context("could not spawn server stdout reader")?;

    Ok(ServerProcess {
        child: Arc::new(Mutex::new(child)),
        addr,
        ready_rx,
    })
}

fn wait_for_server_ready(server: &mut ServerProcess) -> Result<()> {
    let deadline = Instant::now() + SERVER_READY_TIMEOUT;
    loop {
        if Instant::now() >= deadline {
            bail!("timed out after {SERVER_READY_TIMEOUT:?}");
        }
        match server.ready_rx.recv_timeout(Duration::from_millis(250)) {
            Ok(ServerReady::Listening) => {
                // The UDP socket is reservation-based; once the message
                // landed we still pause briefly for the netcode server
                // entity to start accepting connections. Tiny, but
                // skipping it causes the first client to occasionally
                // hit "connection refused".
                wait_for_tcp_canary(server.addr);
                return Ok(());
            }
            Ok(ServerReady::Exited) => bail!("server exited before signalling readiness"),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if let Ok(mut child) = server.child.lock()
                    && let Ok(Some(status)) = child.try_wait()
                {
                    bail!("server process exited with status {status}");
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                bail!("server output stream closed before ready signal")
            }
        }
    }
}

/// The server prints its "listening" line as soon as the UDP socket is
/// reserved, but the Lightyear netcode entity needs another tick or two to
/// finish initialising before it accepts a session. We can't TCP-probe a
/// UDP server, so just sleep a short, fixed window, short enough to feel
/// instant, long enough to let the first `app.update()` complete.
fn wait_for_tcp_canary(addr: SocketAddr) {
    // Burn a couple of frame budgets, ~50 ms at 20 Hz is one server tick.
    let _ = addr;
    thread::sleep(Duration::from_millis(150));
}

/// Per-client side of the test layout: window/monitor index (resolved
/// against the real displays on the client side) plus where the player gets
/// pushed within the world after Welcome.
#[derive(Debug, Clone, Copy)]
struct TestClientLayout {
    /// 0-based index of this client. On a multi-monitor setup it selects the
    /// monitor (0 = leftmost, 1 = next to the right) the client goes
    /// borderless-fullscreen on; on a single monitor it's the tile slot for
    /// the side-by-side windowed fallback. See the client-side
    /// `reposition_test_window_system`.
    window_index: u32,
    /// World-space x offset applied to the predicted player controller as
    /// soon as the snapshot arrives. Positive pushes east, negative west.
    spawn_offset_x: f32,
    /// Yaw (radians) the predicted controller is forced to. Used to make
    /// the two players face each other from boot.
    spawn_yaw: f32,
}

/// Layout for the two test clients. Each is described abstractly (index 0/1);
/// the client resolves the actual display once it can query the monitors,
/// one client per monitor when two are present, side-by-side on one otherwise.
///
/// Yaw convention matches the live mouse-look code (`look.yaw -= delta.x`
/// for "mouse moves right"). On this convention:
/// - yaw = 0 → look toward -Z.
/// - yaw = +π/2 → look toward +X.
/// - yaw = -π/2 → look toward -X.
///
/// So the player on the -X side (player1, offset = -1.25) needs yaw = +π/2
/// to look toward +X (at player2), and player2 mirrors it with yaw = -π/2.
fn test_client_layouts() -> [TestClientLayout; 2] {
    let half_pi = std::f32::consts::FRAC_PI_2;
    [
        TestClientLayout {
            window_index: 0,
            spawn_offset_x: -TEST_PLAYER_OFFSET_X,
            spawn_yaw: half_pi,
        },
        TestClientLayout {
            window_index: 1,
            spawn_offset_x: TEST_PLAYER_OFFSET_X,
            spawn_yaw: -half_pi,
        },
    ]
}

fn spawn_client(
    exe: &std::path::Path,
    server_addr: SocketAddr,
    name: &str,
    account_id: u64,
    layout: TestClientLayout,
) -> Result<Child> {
    // Opt-in headless capture mode (`GAME_TEST_HEADLESS=1 ./cli multiplayer-test`):
    // both clients render off-screen and bind a per-client control socket so an
    // agent can drive + screenshot them (e.g. to verify the third-person rig: one
    // player swinging as seen from the other). The normal GUI dev flow is
    // untouched. See docs/multiplayer-testing.md.
    let headless = std::env::var_os("GAME_TEST_HEADLESS").is_some();
    let mut command = Command::new(exe);
    command
        .arg("client")
        .arg("--connect")
        .arg(server_addr.to_string())
        .env("GAME_PLAYER_NAME", name)
        .env("GAME_ACCOUNT_ID", account_id.to_string())
        // Spawn-placement keys so the two characters land facing each other.
        .env(
            "GAME_TEST_SPAWN_OFFSET_X",
            layout.spawn_offset_x.to_string(),
        )
        .env("GAME_TEST_SPAWN_YAW", layout.spawn_yaw.to_string())
        // Inventory stays closed in headless capture so it never covers the
        // other player in a screenshot; the GUI flow keeps it open.
        .env("GAME_TEST_INVENTORY_OPEN", if headless { "0" } else { "1" })
        // Auto-issue `/test-kit` on join so both windows boot with the
        // full early-game kit. Pairs with the admin account IDs that
        // multiplayer-test seeds into the save before spawning the
        // server.
        .env("GAME_TEST_AUTO_KIT", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        // Pipe stderr (where the Bevy/replication-trace logs go) so it can be
        // prefixed with this client's player name below. Without a per-process
        // tag a two-client trace is ambiguous: a replication-trace line's
        // `client=` field is the PLAYER the mirror represents, not the logging
        // process, and both clients' output interleaves into one stream.
        .stderr(Stdio::piped());
    if headless {
        // Off-screen render + per-client control socket. Deliberately omit the
        // `GAME_TEST_WINDOW_*` geometry keys: those drive the on-screen
        // window-reposition path, which fights the hidden capture window.
        // The harness PID in the socket path keeps two concurrent headless
        // runs from stealing each other's sockets (the temp save dir is
        // already PID-scoped for the same reason); the path is printed so a
        // driving script can pick it up instead of assuming a fixed name.
        let socket = format!(
            "/tmp/ashwend-mptest-{}-{}.sock",
            std::process::id(),
            layout.window_index
        );
        println!(
            "multiplayer-test: client {} control socket at {socket}",
            layout.window_index
        );
        command
            .env("GAME_HEADLESS_CAPTURE", "1280x960")
            .env("GAME_CONTROL_SOCKET", socket);
    } else {
        // On-screen test windows, sized + indexed so the client can place them
        // side-by-side once it can query the real monitor.
        command
            .env("GAME_TEST_WINDOW_WIDTH", TEST_WINDOW_WIDTH.to_string())
            .env("GAME_TEST_WINDOW_HEIGHT", TEST_WINDOW_HEIGHT.to_string())
            .env("GAME_TEST_WINDOW_INDEX", layout.window_index.to_string())
            .env("GAME_TEST_WINDOW_COUNT", "2")
            .env("GAME_TEST_WINDOW_GAP", TEST_WINDOW_GAP.to_string());
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("could not spawn client binary {}", exe.display()))?;
    // Prefix this client's stderr with its player name so a two-client
    // replication trace is unambiguous about which PROCESS logged each line.
    // Mirrors the `[server]` stdout reader above; the thread ends when the child
    // exits and its stderr pipe closes.
    if let Some(stderr) = child.stderr.take() {
        let tag = name.to_owned();
        thread::Builder::new()
            .name(format!("multiplayer-test-client-stderr-{tag}"))
            .spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    eprintln!("[{tag}] {line}");
                }
            })
            .ok();
    }
    Ok(child)
}

fn wait_for_clients(clients: &mut Vec<Child>, exit_signal: Arc<AtomicBool>) {
    while !clients.is_empty() {
        if exit_signal.load(Ordering::SeqCst) {
            for child in clients.iter_mut() {
                let _ = child.kill();
            }
            for child in clients.iter_mut() {
                let _ = child.wait();
            }
            clients.clear();
            return;
        }
        let mut still_running = Vec::with_capacity(clients.len());
        for mut child in std::mem::take(clients) {
            match child.try_wait() {
                Ok(Some(status)) => {
                    println!("multiplayer-test: client exited with {status}");
                }
                Ok(None) => still_running.push(child),
                Err(error) => {
                    eprintln!("multiplayer-test: error polling client: {error}");
                    still_running.push(child);
                }
            }
        }
        *clients = still_running;
        if clients.is_empty() {
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }
}

impl ServerProcess {
    fn shutdown(self) {
        // First try a clean wait, the server has a Ctrl-C handler and the
        // process tree dies when the parent exits, but we still join so we
        // don't leak when the user closed clients gracefully.
        if let Ok(mut child) = self.child.lock() {
            if let Ok(None) = child.try_wait() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
        // TCP probe just keeps the addr in scope for the lifetime of the
        // server, match prevents the field from being warned as dead.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(1));
        // Drain readiness channel to make sure the stdout thread can exit.
        let _ = self.ready_rx;
    }
}

fn ctrlc_listener(flag: Arc<AtomicBool>) {
    let flag_clone = flag.clone();
    let _ = thread::Builder::new()
        .name("multiplayer-test-ctrlc".to_owned())
        .spawn(move || {
            // No external crate dependency. POSIX ignore-SIGINT-and-flag
            // pattern via the standard library only, install a tiny signal
            // shim by spawning a child that re-reads stdin. We have no such
            // shim, so we just busy-wait until the parent's stdin is gone.
            //
            // Best-effort: if a user hits Ctrl-C in the terminal, the
            // signal kills the spawned processes (same process group), and
            // they'll exit on their own. This loop just ensures the helper
            // doesn't get stuck if something weird happens.
            let mut buf = String::new();
            let _ = std::io::stdin().read_line(&mut buf);
            flag_clone.store(true, Ordering::SeqCst);
        });
    let _ = flag;
}

struct TempDir {
    path: std::path::PathBuf,
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn tempdir(prefix: &str) -> Result<TempDir> {
    let mut path = std::env::temp_dir();
    path.push(prefix);
    std::fs::create_dir_all(&path)
        .with_context(|| format!("could not create temp directory {}", path.display()))?;
    Ok(TempDir { path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_names_falls_back_to_defaults_when_unset() {
        assert_eq!(
            resolved_names(None),
            ["player1".to_owned(), "player2".to_owned(),]
        );
    }

    #[test]
    fn resolved_names_applies_partial_overrides() {
        let names = resolved_names(Some(vec!["Tom".to_owned()]));
        assert_eq!(names, ["Tom".to_owned(), "player2".to_owned()]);
    }

    #[test]
    fn resolved_names_ignores_whitespace_overrides() {
        let names = resolved_names(Some(vec!["   ".to_owned(), "Echo".to_owned()]));
        assert_eq!(names, ["player1".to_owned(), "Echo".to_owned()]);
    }

    #[test]
    fn test_client_layouts_are_symmetric_and_offsets_oppose() {
        let [alpha, bravo] = test_client_layouts();
        // Windows are distinct tile slots; the actual pixel position is
        // resolved on the client side once the monitor is known.
        assert_eq!(alpha.window_index, 0);
        assert_eq!(bravo.window_index, 1);

        // Spawn offsets are equal-and-opposite so the players land
        // symmetric around the world spawn point.
        assert!((alpha.spawn_offset_x + bravo.spawn_offset_x).abs() < f32::EPSILON);
        assert!(alpha.spawn_offset_x < 0.0 && bravo.spawn_offset_x > 0.0);

        // Yaws are also equal-and-opposite, facing each other.
        assert!((alpha.spawn_yaw + bravo.spawn_yaw).abs() < f32::EPSILON);
    }
}
