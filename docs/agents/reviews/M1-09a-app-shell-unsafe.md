# Unsafe audit: M1-09a-app-shell (macos.rs)
Auditor: Claude Opus 5.5 (read-only) · 2026-11-12

## Verdict
**sound with changes**: all four `unsafe` blocks are sound for every current caller (confirmed against tauri 2.12.5, tauri-runtime-wry 2.12.3, wry 0.57.2, objc2-foundation 0.3.2, objc2-app-kit 0.3.2); the changes asked for are a cheap main-thread guard in `apply_to_new_window`, whose soundness depends on callers nothing enforces, and some comment fixes.

Checked: `cargo clippy -p atrium-app --all-targets -- -D warnings` is clean, and `cargo test -p atrium-app` passes (41 + 2 tests) on macOS. `unsafe_code = "deny"` is in the workspace `Cargo.toml:45`. The only `#![allow(unsafe_code)]` is `app/src-tauri/src/macos.rs:8`, and that module is declared `#[cfg(target_os = "macos")] mod macos;` (`lib.rs:17-18`). No other file in `app/src-tauri/src` contains an `unsafe` block. The only other match for the word is a comment in `snapshot.rs:101`.

### How the dependencies behave (the basis for the findings below)
- **`with_webview`** (tauri-runtime-wry `lib.rs:1552`) calls `send_user_message` (`lib.rs:263`).
  - On the main thread, it runs `handle_user_message` inline, so the closure runs **synchronously, before `with_webview` returns**.
  - On any other thread, it posts the closure to the event loop, which runs it on the main thread.
- **Inside the handler** (`lib.rs:3903-3912`): the closure gets `Retained::as_ptr(&ns_window)`, where `ns_window` is a local `Retained<NSWindow>` from wry's `ns_window()`.
  - wry's `ns_window()` is `self.webview.webview.window().unwrap()` (wry `lib.rs:2522`), so the pointer is never null.
  - The `Retained` is held until the closure returns.
  - If the window or webview no longer exists, the handler (`lib.rs:3643-3649`) skips the closure, and it is dropped without running.
- **`WebviewWindow::ns_window()`** (tauri `window/mod.rs:1729`) gets the window from the content view's `window().expect(..)` and returns `Retained::autorelease_ptr`. The result is non-null, and it is valid until the current autorelease pool drains.
- **Sync commands**: `#[tauri::command]` without `async` is generated as a blocking body. It runs inline in the IPC handler (`ipc/protocol.rs:75`), which WKWebView's URL-scheme callback invokes on the main thread. `async` commands go through `respond_async_serialized`, which uses `async_runtime::spawn` (a Tokio worker thread).
- **`setup`** runs on the main thread.
- **Thread kinds in objc2-app-kit 0.3.2**: `NSWindow` is `#[thread_kind = MainThreadOnly]`, so a `&NSWindow` is itself a main-thread proof and must not exist off the main thread. `NSWorkspace` has no thread kind, so objc2 treats it as usable from any thread.
- **`addObserverForName_object_queue_usingBlock`** (objc2-foundation `NSNotification.rs:204-218`) lists three safety requirements:
  - `obj` of the correct type;
  - `queue` may have additional threading requirements;
  - the block must be sendable.

## Blocks

### 1. `set_alpha`: `&*webview.ns_window().cast::<NSWindow>()` (macos.rs:62)
This block sets the window's alpha and reads it back, inside a `with_webview` closure. **The SAFETY comment holds:**
- The closure always runs on the main thread, either inline or through the event loop.
- The pointer comes from a `Retained<NSWindow>` that lives across the call, so it is neither null nor dangling.
- The borrow does not escape the closure; only an `f64` is sent out.
- The cast is to the same class that wry stored. The real object is tao's `NSWindow` subclass.

**Caller and thread:**
- The only caller is `native::set_alpha`, called only from the async command `set_window_alpha` (`lib.rs:499`). It runs on a Tokio worker, which blocks in `recv_timeout` while the main thread runs the closure.
- If someone later calls it from the main thread, the closure runs inline, and `send` happens before `recv`. The channel is unbounded, so this returns immediately: **no deadlock and no 5-second stall**.
- If the window has been destroyed (a pop-out closing), the closure is dropped and the sender with it. `recv_timeout` then returns `Disconnected` at once, and the pointer is never touched.

### 2. `window_number`: `&*webview.ns_window().cast::<NSWindow>()` (macos.rs:80)
This block reads `windowNumber`. Same mechanism as block 1, and **the SAFETY comment holds**.

**Caller and thread:** the only caller is `snapshot::take`, from the **sync** command `take_snapshot` (`lib.rs:665`), so it runs on the main thread. `with_webview` therefore runs the closure inline, and the value is already in the channel when `recv_timeout` is called. There is no deadlock and no stall; I checked the `send_user_message` main-thread branch to confirm this.

### 3. `apply_to_new_window`: `&*pointer.cast::<NSWindow>()` (macos.rs:106)
This block dereferences the autoreleased pointer from `WebviewWindow::ns_window()` and sets the alpha. **The SAFETY comment holds for every current caller**, all on the main thread:
- `setup`, through `apply_native` (`lib.rs:883`);
- the sync `pop_out` → `open_popout` (`lib.rs:403-407`, `380`);
- the sync `restore_windows` → `open_popout` (`lib.rs:459-470`).

**Why the pointer is valid:**
- It is non-null; tauri `expect`s the window.
- It is valid for the borrow, which ends before the function returns. On the main thread there is always an autorelease pool, and it does not drain inside this call.
- The window was built a moment earlier on the same thread, by `build_main`/`build_popout` → `builder.build()`, and nothing between can close it.

**Where it is weaker than blocks 1 and 2:** the main-thread requirement is only a doc comment (`native.rs:115-116`) and a code comment. Nothing checks it at runtime, unlike the `with_webview` path, which routes to the main thread on its own. See finding 1.

### 4. `observe_accessibility`: `addObserverForName_object_queue_usingBlock` (macos.rs:126)
This block registers a block on `NSWorkspace`'s notification center for `NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification`, with no object filter and no queue. **The SAFETY comment holds.** Against the three documented requirements:
- **`obj`**: it is `None`, so the type requirement is met trivially.
- **`queue`**: it is `None`, so the block runs synchronously on whichever thread posts the notification. In practice that is the main thread, but nothing relies on it: the block does not touch any `MainThreadOnly` object.
- **The block is sendable**:
  - It captures only an `AppHandle`, which is `Send + Sync + 'static` (Tauri moves it across threads everywhere).
  - It calls `app.emit(..)`, which takes `&self` and is thread-safe.
  - It calls `accessibility_opaque()`, which uses `NSWorkspace`, a class with no thread kind.
  - The block is `Fn`, not `FnMut`, so concurrent invocation would also be fine.

**Lifetime:**
- `RcBlock` is a heap block. The notification center copies/retains it, so dropping the local `block` at the end of the function is fine.
- `mem::forget(token)` leaks one retain of the observer token. The observer stays registered (and the `AppHandle` stays alive) for the life of the process, which is the stated intent.
- Leaking a `Retained` is safe in Rust and in ObjC.
- `observe_accessibility` is called once, in `setup` (`lib.rs:885`), so there is exactly one observer.

### Safe calls in the module
- `NSWorkspace::sharedWorkspace()`, `notificationCenter()`, `accessibilityDisplayShould*()` and the `window_vibrancy` calls are safe APIs. I found nothing among them that is actually unsound.
- `set_vibrancy` is called from the sync command `set_vibrancy` (`lib.rs:484`) and from `apply_to_new_window`. Both run on the main thread, which `window_vibrancy` itself also requires and checks.

## Findings

1. [minor] **`apply_to_new_window`'s soundness rests on an unenforced main-thread precondition**
   - **Where:** `app/src-tauri/src/macos.rs:100-109` (and `native.rs:117`, a safe `pub fn`).
   - **Problem:**
     - `native::apply_to_new_window` is a safe public function. If a future caller is an `async` command, or code in a spawned task, it creates `&NSWindow` on a Tokio worker. That breaks objc2's `MainThreadOnly` invariant, and `setAlphaValue` then runs off the main thread, which is undefined behaviour in AppKit.
     - Off the main thread, the autoreleased pointer from `ns_window()` may also not be covered by an autorelease pool.
     - Turning `pop_out` or `restore_windows` into `async fn` is a one-word change that the compiler would accept.
   - **Fix:** guard the block with `let Some(_mtm) = objc2::MainThreadMarker::new() else { eprintln!(..); return applied; };`. Alternatively, `debug_assert!` plus an early return. Then the SAFETY comment states a checked fact, not a property of the callers. The rest of the function (vibrancy) already goes through `window_vibrancy`, which checks the thread itself.

2. [nit] **The module doc is out of date**
   - **Where:** `app/src-tauri/src/macos.rs:4-7`.
   - **Problem:**
     - It says "two dereferences of the `NSWindow` pointer"; there are three: `set_alpha`, `window_number` and `apply_to_new_window`.
     - It says the module "has not been compiled since" the spike. It compiles and passes clippy on macOS, and CR-11 records that it ran.
   - **Fix:** say three dereferences plus one observer registration, and drop the "not compiled" sentence.

3. [nit] **The observer's SAFETY comment doesn't name the "block must be sendable" requirement**
   - **Where:** `app/src-tauri/src/macos.rs:124-125`.
   - **Problem:** the comment implies it ("safe from any thread") but doesn't list it next to the other documented requirements. It also doesn't say that `queue: None` means the block runs on the posting thread. A later edit that captures a `MainThreadOnly` value (for example, a window) would break the requirement silently.
   - **Fix:** reword it to cover all three requirements. For example: "obj is None; queue is None, so the block runs on the posting thread, which may be any; the block captures only an `AppHandle` (Send + Sync) and touches no MainThreadOnly object, so it is sendable." Optionally, add a compile-time `fn _assert<T: Send + Sync>() {}` check on `AppHandle`.

4. [minor] **`set_alpha` can apply the alpha after reporting failure** (not a soundness issue)
   - **Where:** `app/src-tauri/src/macos.rs:67-69`, with `lib.rs:504-508`.
   - **Problem:** if the main thread is busy for more than 5 s, `set_window_alpha` returns `Err` and leaves `state.native` unchanged. The queued closure still runs later and changes the window's alpha. The recorded native state then disagrees with the window until the next change.
   - **Fix:** accept this, since the frontend reconciles. Or note it in the doc comment.
