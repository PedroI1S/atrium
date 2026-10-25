//! Live tests against a real Chromium (Helium on the dev machine). Each test launches its
//! own headless browser with a throwaway profile and serves the fixture pages itself.
//! They skip (and say so) when no browser is installed or `ATRIUM_SKIP_LIVE=1`.

use std::sync::Arc;
use std::time::Duration;

use atrium_browser_screencast_lib::browser::{Browser, KeyParams, LaunchOptions, MouseParams, Page, ScreencastOptions};
use atrium_browser_screencast_lib::discovery;
use atrium_browser_screencast_lib::fixture::Fixtures;
use serde_json::{Value, json};

struct Live {
    browser: Arc<Browser>,
    fixtures: Fixtures,
    _dir: tempfile::TempDir,
}

async fn live() -> Option<Live> {
    if std::env::var_os("ATRIUM_SKIP_LIVE").is_some() {
        eprintln!("SKIP: ATRIUM_SKIP_LIVE set");
        return None;
    }
    let Some(info) = discovery::pick(&discovery::discover(), None) else {
        eprintln!("SKIP: no Chromium-based browser installed");
        return None;
    };
    let dir = tempfile::tempdir().unwrap();
    let browser = Browser::launch(&LaunchOptions {
        binary: info.binary,
        user_data_dir: dir.path().join("profile"),
        log_path: Some(dir.path().join("stderr.log")),
        extra_args: vec![],
    })
    .await
    .expect("launch");
    let fixtures = Fixtures::start().await.unwrap();
    Some(Live { browser, fixtures, _dir: dir })
}

async fn read_state(page: &Page) -> Value {
    page.eval("JSON.stringify(window.__read())").await.map(|v| serde_json::from_str(v.as_str().unwrap()).unwrap()).unwrap()
}

async fn echo_cookie(page: &Page, live: &Live) -> String {
    page.eval(&format!("fetch('{}').then(r => r.text())", live.fixtures.url("/echo-cookie"))).await.unwrap().as_str().unwrap().to_string()
}

/// The core claim of PLAN §3.13: one browser process, one context per thread, and no state
/// shared between them even though both pages are on the same origin.
#[tokio::test]
async fn contexts_do_not_share_cookies_or_storage() {
    let Some(l) = live().await else { return };
    let a = l.browser.new_page(&l.fixtures.url("/state.html")).await.unwrap();
    let b = l.browser.new_page(&l.fixtures.url("/state.html")).await.unwrap();
    assert_ne!(a.context_id, b.context_id);

    // A writes a JS cookie, localStorage, sessionStorage and a server-set cookie.
    a.navigate(&l.fixtures.url("/state.html?set=A")).await.unwrap();
    a.eval(&format!("fetch('{}')", l.fixtures.url("/set-cookie?v=A"))).await.unwrap();
    let sa = read_state(&a).await;
    assert_eq!(sa["local"], "A");
    assert_eq!(sa["session"], "A");
    assert!(sa["cookie"].as_str().unwrap().contains("who=A"));
    assert!(echo_cookie(&a, &l).await.contains("srv=A"), "A sends its server cookie");

    // B reloads the same origin and sees none of it.
    b.navigate(&l.fixtures.url("/state.html")).await.unwrap();
    let sb = read_state(&b).await;
    assert_eq!(sb["local"], Value::Null, "localStorage leaked: {sb}");
    assert_eq!(sb["session"], Value::Null);
    assert_eq!(sb["cookie"], "", "JS cookie leaked: {sb}");
    assert_eq!(echo_cookie(&b, &l).await, "", "server cookie leaked into B's requests");

    // And the other way round: B writes, A keeps its own values.
    b.navigate(&l.fixtures.url("/state.html?set=B")).await.unwrap();
    b.eval(&format!("fetch('{}')", l.fixtures.url("/set-cookie?v=B"))).await.unwrap();
    a.navigate(&l.fixtures.url("/state.html")).await.unwrap();
    let sa = read_state(&a).await;
    assert_eq!(sa["local"], "A");
    assert!(sa["cookie"].as_str().unwrap().contains("who=A") && !sa["cookie"].as_str().unwrap().contains("who=B"));
    assert!(echo_cookie(&a, &l).await.contains("srv=A") && !echo_cookie(&b, &l).await.contains("srv=A"));
    assert!(echo_cookie(&b, &l).await.contains("srv=B"));

    // Both contexts live in the one browser process we started.
    assert!(!l.browser.cdp.is_closed());
    l.browser.close().await;
}

#[tokio::test]
async fn screencast_streams_frames_and_input_reaches_the_page() {
    let Some(l) = live().await else { return };
    let page = l.browser.new_page(&l.fixtures.url("/click.html")).await.unwrap();
    page.set_viewport(800, 500, 1.0).await.unwrap();
    let mut frames = page.start_screencast(ScreencastOptions::default()).await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), frames.next()).await.expect("first frame").unwrap();
    assert_eq!(&first.data[..2], &[0xff, 0xd8], "JPEG magic");
    assert_eq!(first.meta.device_width as u32, 800);
    page.ack(first.ack_id);

    let mouse = |kind: &str| MouseParams {
        kind: kind.into(),
        x: 60.0,
        y: 60.0,
        button: Some("left".into()),
        buttons: Some(i32::from(kind == "mousePressed")),
        click_count: Some(1),
        modifiers: None,
        delta_x: None,
        delta_y: None,
    };
    page.mouse(&mouse("mousePressed")).await.unwrap();
    page.mouse(&mouse("mouseReleased")).await.unwrap();
    assert_eq!(page.eval("window.__clicks").await.unwrap(), 1);

    // The changed frame arrives and its probe square is red (decoded, not assumed).
    let mut red = false;
    for _ in 0..30 {
        let Ok(Some(f)) = tokio::time::timeout(Duration::from_secs(2), frames.next()).await else { break };
        let mut d = jpeg_decoder::Decoder::new(&f.data[..]);
        let px = d.decode().unwrap();
        let info = d.info().unwrap();
        let i = (60 * info.width as usize + 60) * 3;
        if px[i] > 200 && px[i + 1] < 60 && px[i + 2] < 60 {
            red = true;
            break;
        }
        page.ack(f.ack_id);
    }
    assert!(red, "no frame showed the red probe after the click");
    page.stop_screencast().await.unwrap();
    l.browser.close().await;
}

#[tokio::test]
async fn keyboard_ime_and_wheel_are_forwarded() {
    let Some(l) = live().await else { return };
    let page = l.browser.new_page(&l.fixtures.url("/form.html")).await.unwrap();
    page.set_viewport(800, 500, 1.0).await.unwrap();
    page.eval("document.getElementById('text').focus()").await.unwrap();

    // Plain typing: keyDown with text produces the character.
    for ch in ["h", "i"] {
        let down = KeyParams {
            kind: "keyDown".into(),
            key: Some(ch.into()),
            code: Some(format!("Key{}", ch.to_uppercase())),
            text: Some(ch.into()),
            unmodified_text: Some(ch.into()),
            windows_virtual_key_code: Some(i32::from(ch.as_bytes()[0].to_ascii_uppercase())),
            modifiers: None,
            auto_repeat: None,
            location: None,
            commands: None,
        };
        page.key(&down).await.unwrap();
        page.key(&KeyParams { kind: "keyUp".into(), text: None, unmodified_text: None, ..down }).await.unwrap();
    }
    // IME: two candidate updates, then the commit.
    page.ime_set_composition("に", 1, 1).await.unwrap();
    page.ime_set_composition("日本", 2, 2).await.unwrap();
    page.insert_text("日本").await.unwrap();
    let st: Value = page.eval("JSON.stringify(window.__state())").await.map(|v| serde_json::from_str(v.as_str().unwrap()).unwrap()).unwrap();
    let log = st["log"].to_string();
    assert!(log.contains("compositionstart") && log.contains("compositionupdate"), "composition events missing: {log}");
    assert!(st["text"].as_str().unwrap().starts_with("hi"), "typing: {}", st["text"]);
    assert!(st["text"].as_str().unwrap().contains("日本"), "IME commit: {}", st["text"]);
    assert!(!st["text"].as_str().unwrap().contains("に"), "composition text must not remain: {}", st["text"]);

    // Wheel scrolls a long page.
    page.navigate(&l.fixtures.url("/scroll.html")).await.unwrap();
    let wheel = MouseParams {
        kind: "mouseWheel".into(),
        x: 200.0,
        y: 200.0,
        button: None,
        buttons: None,
        click_count: None,
        modifiers: None,
        delta_x: Some(0.0),
        delta_y: Some(300.0),
    };
    page.mouse(&wheel).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let y = page.eval("window.__scroll()").await.unwrap().as_f64().unwrap();
    assert!(y > 100.0, "wheel scrolled {y}");

    // Resizing changes the layout viewport.
    page.set_viewport(500, 300, 1.0).await.unwrap();
    assert_eq!(page.eval("innerWidth").await.unwrap(), json!(500));
    l.browser.close().await;
}

/// A second Atrium on the same profile must be refused, and must not touch the first one's browser.
#[tokio::test]
async fn second_owner_of_a_profile_is_refused_and_the_first_browser_survives() {
    let Some(l) = live().await else { return };
    let info = discovery::pick(&discovery::discover(), None).unwrap();
    let err = Browser::launch(&LaunchOptions {
        binary: info.binary,
        user_data_dir: l.browser.user_data_dir.clone(),
        log_path: None,
        extra_args: vec![],
    })
    .await
    .err()
    .expect("second launch must fail");
    assert!(err.to_string().contains("in use by another running Atrium"), "{err}");
    // The first browser is untouched and still answers.
    assert!(l.browser.version().await.unwrap().contains("Chrome"));
    l.browser.close().await;
}
