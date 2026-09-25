//! Cookie isolation in real web views (WKWebView, WebView2, WebKitGTK). Needs a display, so
//! it only runs when asked:
//!
//! ```sh
//! KUBYL_TEST_WEBVIEW=1 cargo test -p kubyl_webview --test live_webview
//! ```
//!
//! Opens a GPUI window with three embedded views on the data stores Kubyl gives Grafana on two
//! clusters (`kind-a` twice, `kind-b` once) and a local HTTP server: a login on cluster A sets a
//! cookie; a second tab on cluster A sees it, a tab on cluster B doesn't. (The forwards don't
//! matter here: cookies ignore ports, which is why each cluster needs its own store.)

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpListener;
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc::{self, UnboundedReceiver};
use gpui::{
    App, AppContext as _, Bounds, Context, IntoElement, Render, Window, WindowBounds,
    WindowOptions, div, point, px, size,
};
use kubyl_webview::native::{NativeEvent, NativeOptions, NativeWebView, ParentWindow, Storage};
use kubyl_webview::store::storage_id;
use kubyl_webview::target::TargetKind;

/// Serves `/set` (sets a session cookie) and `/get` (shows `document.cookie` as the title).
fn serve() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut reader = BufReader::new(&stream);
                let mut request = String::new();
                if reader.read_line(&mut request).is_err() {
                    return;
                }
                // Drain the headers.
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                    line.clear();
                }
                let (cookie, body) = if request.starts_with("GET /set") {
                    (
                        "Set-Cookie: grafana_session=signed-in; Path=/; SameSite=Lax\r\n",
                        "<!doctype html><title>set</title>".to_string(),
                    )
                } else {
                    (
                        "",
                        "<!doctype html><title>loading</title><script>document.title = 'cookie:' + document.cookie</script>"
                            .to_string(),
                    )
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n{cookie}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let mut stream = stream;
                stream.write_all(response.as_bytes()).ok();
            });
        }
    });
    port
}

struct Empty;

impl Render for Empty {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

struct View {
    native: NativeWebView,
    events: UnboundedReceiver<NativeEvent>,
}

fn view(parent: &ParentWindow, id: [u8; 16], url: String, index: usize) -> View {
    let (tx, rx) = mpsc::unbounded();
    let dir = std::env::temp_dir().join("kubyl-live-webview");
    let native = NativeWebView::create(
        parent,
        NativeOptions {
            url,
            title: format!("view {index}"),
            storage: Storage::Isolated { id },
            data_dir: dir.clone(),
            staging_dir: dir.join("downloads"),
            devtools: false,
            zoom: 1.0,
            accepted_certs: Default::default(),
            shortcuts: Default::default(),
        },
        tx,
    )
    .expect("web view");
    native.set_bounds(Bounds::new(
        point(px(10.0 + 210.0 * index as f32), px(40.0)),
        size(px(200.0), px(200.0)),
    ));
    native.set_visible(true);
    View { native, events: rx }
}

/// The next title of `view` that isn't a loading placeholder.
async fn title(view: &mut View) -> String {
    loop {
        match view.events.next().await {
            Some(NativeEvent::TitleChanged(title)) if title != "loading" && !title.is_empty() => {
                return title;
            }
            Some(_) => {}
            None => panic!("the web view went away"),
        }
    }
}

fn fail(message: String) -> ! {
    eprintln!("live_webview: FAILED: {message}");
    std::process::exit(1);
}

fn main() {
    if std::env::var_os("KUBYL_TEST_WEBVIEW").is_none() {
        println!("live_webview: skipped (set KUBYL_TEST_WEBVIEW=1; needs a display)");
        return;
    }
    let port = serve();
    let base = format!("http://127.0.0.1:{port}");
    // Fresh stores every run: WebKit keeps them on disk.
    let nonce = std::process::id().to_string();
    let store = |context: &str| {
        storage_id(
            &format!("{context}-{nonce}"),
            Some("https://127.0.0.1:6443"),
            "monitoring",
            TargetKind::Service,
            "grafana",
        )
    };
    let (cluster_a, cluster_b) = (store("kind-a"), store("kind-b"));
    assert_ne!(cluster_a, cluster_b);

    gpui_platform::application().run(move |cx: &mut App| {
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(100.0), px(100.0)),
                        size(px(680.0), px(280.0)),
                    ))),
                    ..Default::default()
                },
                |_, cx| cx.new(|_| Empty),
            )
            .expect("window");
        let parent = window
            .update(cx, |_, window, _| ParentWindow::of(window))
            .expect("window")
            .expect("window handle");
        let timeout = cx.background_executor().clone();
        // WebKitGTK runs in GTK's main loop (Kubyl pumps it the same way).
        if kubyl_webview::native::NEEDS_PUMP {
            cx.spawn(async move |cx| {
                loop {
                    kubyl_webview::native::pump();
                    cx.background_executor()
                        .timer(Duration::from_millis(8))
                        .await;
                }
            })
            .detach();
        }
        cx.spawn(async move |cx| {
            // Web view creation pumps the Win32 message loop: outside of any App update.
            let mut signed_in = view(&parent, cluster_a, format!("{base}/set"), 0);
            let test = async {
                let set = title(&mut signed_in).await;
                if set != "set" {
                    fail(format!("the sign-in page said {set:?}"));
                }
                signed_in.native.load_url(&format!("{base}/get"));
                let own = title(&mut signed_in).await;
                let mut same_cluster = view(&parent, cluster_a, format!("{base}/get"), 1);
                let shared = title(&mut same_cluster).await;
                let mut other_cluster = view(&parent, cluster_b, format!("{base}/get"), 2);
                let isolated = title(&mut other_cluster).await;
                println!("cluster A, signed in:  {own}");
                println!("cluster A, second tab: {shared}");
                println!("cluster B:             {isolated}");
                if own != "cookie:grafana_session=signed-in" {
                    fail(format!("the cookie wasn't set: {own:?}"));
                }
                if shared != own {
                    fail(format!("a second tab on the same cluster got {shared:?}"));
                }
                if isolated != "cookie:" {
                    fail(format!("the other cluster's store saw {isolated:?}"));
                }
            };
            let deadline = timeout.timer(Duration::from_secs(60));
            futures::pin_mut!(test, deadline);
            if let futures::future::Either::Right(_) = futures::future::select(test, deadline).await
            {
                fail("timed out".into());
            }
            println!("live_webview: ok: cookies are isolated per cluster");
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}
