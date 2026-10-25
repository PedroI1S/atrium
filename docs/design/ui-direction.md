# Atrium UI direction

Status: **chosen by Pedro, 2026-10-25.** This is the spec for M1's UI work (PLAN §3.16, §3.20). It replaces the three M0-06 directions (Glass, Flat, Paper), which Pedro rejected. The token architecture and the layout mechanics from M0-06 stay as they are (`docs/contracts/m0.md` §4).

- **Layout:** B, *Floating spaces*. Use C's centered file path in the editor header, and keep B's inbox list (not C's one-at-a-time triage).
- **Prototype:** [Atrium UI directions](https://claude.ai/artifact/AwFdtq8w8ibPrq2iJk3mNQ), artboard "B · Floating spaces". C is kept there for reference. Use the pill bar above the window to switch screens, theme and opacity.
- **Draft themes:** [`themes/atrium-calm-light.toml`](themes/atrium-calm-light.toml) and [`themes/atrium-calm-dark.toml`](themes/atrium-calm-dark.toml).
- **Contract changes it implies:** CR-02 in `docs/agents/change-requests.md` (a proposal, for the orchestrator).

Input: D-023 (Pedro's requirements). Design-session answers from Pedro:
- what was wrong with M0-06: "too much at once" and a "generic look";
- references: Things / Bear / Craft for calm, Arc / Zed for minimal chrome;
- opacity: both modes, user picks;
- colour: near-monochrome.

## 1. Principles

1. **Calm by default.**
   - Greys and one ink accent.
   - Colour appears only where it carries meaning: attention states, diffs, syntax.
   - No heavy borders. Structure comes from surface steps, spacing and soft card shadows.
2. **Show one thing well.**
   - The main card holds the current task.
   - Everything else is one click away and minimizable: the topic list on the left, context panels on the right.
   - The default window shows at most three regions.
3. **Progressive disclosure.**
   - Every surface has a simple level and a deeper one, and the deeper level only appears when asked for. Examples: the Advanced pane, minimized panels, theme files.
   - Deep customization (tokens, custom CSS, VS Code themes) is reachable from Advanced and nowhere else.
4. **Nothing hard-coded.**
   - Every value below is a token (contract §4).
   - The prototype is built the same way: switching theme, tint or opacity only changes custom properties.

## 2. Window anatomy (layout B)

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ ● ● ●                                                     window frame (s-0) │
│ ┌ sidebar 244 ─────┐ ┌ main card ────────────────────────┐ ┌ panel 420 ────┐ │
│ │ ⌘K  Ask, search… │ │ Title · state pill      chips  ⧉  │ │ Tabs…       » │ │
│ │ [Inbox] [Home]   │ │                                   │ │               │ │
│ │ Auth & sessions ⋯│ │   thread (measure 640)            │ │ Changes /     │ │
│ │ + New chat       │ │   tool steps, approval card       │ │ Checks /      │ │
│ │ ● Rework token…  │ │                                   │ │ Files /       │ │
│ │ ● Keychain…      │ │                                   │ │ Terminal …    │ │
│ │                  │ │   composer                        │ │               │ │
│ │ (Au)(Da)(Do)(Id)+ ⚙ │                                   │ │               │ │
│ └──────────────────┘ └───────────────────────────────────┘ └───────────────┘ │
└──────────────────────────────────────────────────────────────────────────────┘
```

- **Frame** (`--surface-0`): the window background.
  - The sidebar sits directly on it, with no card around it.
  - This is the layer that turns see-through first when opacity is lowered.
- **Cards** (`--surface-1`, `--card-radius` 11 px, `--shadow-card`): the main card and the right panel. They float with an 8 px gap (`--card-gap`).
- **Traffic lights:** inline at top-left (Tauri `titleBarStyle: "Overlay"`, as in M0-06). The strip above the sidebar is a drag region.
- **Sizes** (component tokens):
  - sidebar 244 px;
  - right panel 420 px (380 px when the main card is the editor);
  - minimized strips 46 px.
  - All are resizable by dragging, and the sizes are remembered per workspace.

### 2.1 Left sidebar

From top to bottom:
1. A hide-sidebar button.
2. The ⌘K bar ("Ask, search or jump").
3. Two pinned tiles:
   - **Inbox**, showing the needs-you count in the needs-you colour;
   - **Home**.
4. The **current topic's name** (with a ⋯ menu), "New chat in this topic", and that topic's chats. Each chat row shows a status dot, title and age. The selected chat is drawn as a small raised card.
5. At the bottom, the **topic switcher**:
   - one circle per topic, showing a two-letter monogram;
   - `+` for a new topic;
   - Settings.
   - A topic with something that needs you gets a small amber dot.

**Hidden** (⌘B), the sidebar collapses to a strip: show-sidebar, Inbox (with a dot), and the topic circles.

### 2.2 Right panel (minimizable)

- Its tabs depend on what the main card shows:

  | Main card | Panel tabs | Default |
  |---|---|---|
  | Chat | Changes · Checks · Files · Terminal (Browser and Database added in M1) | Changes, when the agent edited files |
  | Editor | Chat · Files · Changes | Chat |
  | Inbox | Preview of the selected item | Preview |
  | Settings | No panel; the Advanced card takes its place | — |

- **Minimize** (» in the panel header, ⌘⌥B) collapses it to a 46 px strip of icons:
  - expand, Changes, Checks, Files, Terminal, Browser, Database;
  - a 7 px dot means something is waiting there (ready = green, needs you = amber);
  - clicking an icon reopens the panel on that tab.
- Clicking an inline tool step opens the matching tab: Edit → Changes, Run → Terminal.
- Open/minimized state and the active tab are remembered per main-card kind, per workspace.

## 3. Screens

### Chat

- **Header:** title, state pill (e.g. *Needs you*), worktree chip, and ⧉ *Open in new window*.
- **Thread:** centered at a 640 px measure, 14.5 px text.
  - User messages: a soft bubble on the right (`--bubble-bg`).
  - Agent text: plain, no bubble.
  - **Tool steps:** compact one-line rows (icon, verb, target in mono, result on the right, e.g. "14 passed ✓", "+5 −3"). Clicking one opens the right tab in the panel.
  - **Approval card:**
    - an outlined card with an amber dot and the question ("Run this command?");
    - the command in mono, and one line of risk ("No network");
    - `Allow once ↵` (primary), `Always in this worktree`, and `Deny esc` (ghost).
- **Composer:** a rounded field with chips for the approval mode and the agent/model, plus a round send button.

### Inbox

- **One ranked list across all topics** (PLAN §3.4), grouped as:
  1. approvals and questions,
  2. failed,
  3. ready for review.
- Each item shows a dot, a one-line ask, its source (chat · topic · agent) and its age.
  - **Approvals** have their buttons on the item.
  - **Questions** render their answers as buttons (`Yes, both` / `60 Hz only` / `Answer in chat`).
- **Selecting an item** shows its preview in the right panel: a summary, checks, and *Open chat* / *See changes*.
- **Filter:** All · Approvals · Reviews.
- **Footer hint:** J / K to move, ↵ to open, A to allow, D to deny, and ⌘J to jump here from anywhere.

### Editor

- **Header:** file tabs on the left (a modified-file dot), **the file path centered** (`token-refresh / crates/atrium-core/src / session.rs`, file name bold), then a hint chip `⌘K edit · ⌘L ask` and ⧉.
- **Code:** CodeMirror 6, 13 px Geist Mono, 23 px line height.
  - The current line is tinted with `--hover-bg`.
  - Editor AI suggestions show as `--ghost-text`.
- **⌘K inline edit** (PLAN §3.21):
  - a small raised bar right under the line, holding the instruction, the model, `Accept ⌘↵` and `Reject`;
  - the proposed change shows below it as an inline diff.
- **Right panel:** the chat for this worktree, in compact form (13 px). Its composer is scoped to the selection ("Ask about this file…", with an `L43–52` chip).
- **Status bar:** language, cursor position, the hint "Tab accepts the grey suggestion", and the editor AI badge (e.g. `● Copilot · Auto`). The badge is hidden while editor AI is off.

### Settings

- **Settings opens as a card in place of the main card.** The sidebar stays, so settings never takes over navigation.
- **Sections** are tabs across the card header: General · Appearance · Agents · Approvals · Notifications · More (features such as Editor AI, Databases and Sandbox live under More). There is a search button as well.
- **Each section shows only its essentials:** at most about five rows, each a label and a one-line hint, with one control on the right (segmented, toggle, slider or select).
- **"Advanced \<section\> ›"** is the last row. It opens the **Advanced card** (400 px) in the right panel's place, with a breadcrumb ("Appearance / Advanced") and a close button.
  - It is never a modal or a popover.
  - It closes when you leave the section.
- **Extensible:** a feature registers a section with `essentials[]` and `advanced[]`. If it has more than about five essentials, the rest go to Advanced. **Settings search covers Advanced too** and opens the pane on the match.

Appearance contents, as the reference for every section:

| Essentials | Advanced |
|---|---|
| Theme: Light / Dark / System | Opacity applies to: Background / Whole window |
| Window opacity (slider, 30–100 %) | Keep reading areas solid (toggle) |
| Blur behind the window (toggle) | Blur strength (0–48 px) |
| Text size: Small / Default / Large | Popped-out windows: Same as main / Always opaque |
| | Tint: Neutral / Warm / Cool |
| | Card gap: None / 8 / 12 |
| | Fonts (UI · code) |
| | Theme file: import VS Code, edit tokens, `~/.config/atrium/theme.css` |

### Popped-out windows

- **Any chat, editor or database view can become its own window** (D-023). You can do it with:
  - ⧉ in the view's header;
  - dragging the card out of the window;
  - the command palette (*Move to New Window*).
- **Window chrome** is minimal: traffic lights, the title, a status pill, a 📌 *keep on top* toggle, and ↩ *move back into the main window*.
- **Closing a popped-out window** puts the view back where it came from in the main window, so nothing is lost.
- A view lives in one place at a time. While a view is popped out, its place in the main window shows a small "Open in its own window · Bring back" placeholder.
- Popped-out windows follow the main window's opacity unless Advanced says otherwise.
- Their positions are remembered per workspace and restored on relaunch.
- Every popped-out window is a Tauri `WebviewWindow` talking to the same daemon client (see §8).

## 4. Topic groups

- A **topic** is a user-made group of chats, e.g. *Auth & sessions*, *Daemon*, *Docs & release*, *Ideas*.
  - Topics can span workspaces. Each chat still shows its worktree as a chip.
  - A chat belongs to one topic.
- **New chats** start in the current topic. Chats created by Home or by agents (delegation) go to the parent chat's topic, or to *Unsorted*.
- **Managing topics:**
  - drag chats between topics, or use *Move to topic…* in the palette;
  - rename, reorder, set the monogram, or archive a topic from its ⋯ menu;
  - ⌃1…⌃9 switch topics.
- **Attention:** a topic circle gets an amber dot when one of its chats needs you, and the inbox still lists everything across topics.
- These are proposed defaults: Pedro picked the layout, not these rules. Confirm or adjust them in M1's UI brief.

## 5. Opacity

The setting Pedro called fundamental. It has two modes and the user picks.

- **Background only** (the default mode):
  - surfaces turn translucent and text, icons and controls stay fully opaque;
  - the frame and sidebar (`surface-0`) use alpha `a`, the slider value;
  - cards use `a + (1 − a) × 0.35`, so reading areas stay more solid than the frame;
  - with *Keep reading areas solid*, cards stay at 1 and only the frame fades.
- **Whole window:** everything fades together, like a terminal's opacity setting. Surfaces stay opaque and the window's CSS `opacity` is `a`.
- **Defaults and limits:**
  - opacity 100 % (so it is off until the user moves it), blur on (it only matters below 100 %), 24 px;
  - range 30–100 %; the lower bound keeps text readable over a busy desktop.
- **Accessibility:**
  - if the OS asks for *Reduce transparency* or *Increase contrast*, Atrium draws opaque surfaces and shows a note in the setting;
  - the high-contrast theme ignores opacity.
- **Platform:**
  - macOS needs `transparent: true` + `macOSPrivateApi` (as M0-06 found). That keeps Atrium out of the Mac App Store, a distribution decision for the orchestrator and Pedro;
  - native blur comes from `window-vibrancy`; CSS `backdrop-filter` blurs the content behind the window's own cards;
  - on Linux, opacity needs a compositor that supports transparent windows, and blur falls back to none (M0-06 §6). If neither works, the slider is disabled with an explanation, not hidden.
- **Applied at runtime, not by themes.** The app sets `--window-alpha-chrome`, `--window-alpha-content`, `--window-opacity` and `--window-blur` from settings, and components compose them with opaque theme surfaces (CR-02). A theme never ships translucency except `surface-glass`.

## 6. Visual language

**Colour**

- Near-monochrome neutral greys, with **ink as the accent**: primary buttons, send, toggles and focus are near-black in light mode and near-white in dark mode.
- Colour only appears for:

  | Meaning | Light | Dark | Used for |
  |---|---|---|---|
  | Needs you (`state-needs-you`) | `#94540f` | `#e0a35c` | approvals, questions, topic dots, inbox count |
  | Failed (`state-failed`) | `#ac3c33` | `#e8867b` | failed checks |
  | Ready (`state-ready`) | `#33704a` | `#80c396` | ready for review, passed checks |
  | Running (`state-running`) | `#4d6684` | `#8fa5c3` | a pulsing dot (static under reduced motion) |

- State is never shown by colour alone: there's always a dot plus a label or a position in the list.
- Pills use the state colour on a 11–13 % tint of itself. They pass AA on `surface-1`; on the frame, use plain coloured text instead.
- **Surfaces:**

  | | Light | Dark |
  |---|---|---|
  | frame (`surface-0`) | `#eeeeeb` | `#161615` |
  | cards (`surface-1`) | `#ffffff` | `#212120` |
  | raised inside cards (`surface-2`) | `#f7f7f5` | `#272725` |

  - Hover and selection are translucent overlays (`--hover-bg`, `--selected-bg`) so they stay right at any opacity.
- **Tints** (Advanced): *Neutral* is the default. *Warm* and *Cool* ship as sibling theme files with the same token names, which is not a contract change. Values from the prototype:

  | | Warm light | Warm dark | Cool light | Cool dark |
  |---|---|---|---|---|
  | `surface-0` | `#f0ece5` | `#181613` | `#ebeef2` | `#131518` |
  | `surface-1` | `#fffefb` | `#23211d` | `#ffffff` | `#1e2125` |
  | `surface-2` | `#f8f5f0` | `#292621` | `#f6f8fa` | `#23272c` |
  | `text` | `#25221d` | `#ece8e0` | `#1c1f24` | `#e6e8ec` |
  | `text-muted` | `#675f54` | `#aaa397` | `#5d636d` | `#9fa6ae` |

- **Contrast:** in both draft themes, `text` and `text-muted` pass WCAG AA on `surface-0..2`, as do `text-on-accent`, every state colour, both diff text colours and every syntax colour on `surface-1`.
  - The lowest are text-muted on surface-0 at 5.2:1 (light), syntax comments at 4.6:1 (dark) and the state colours at about 5:1 on the light frame.
  - `text-faint` is for decoration only (line numbers, idle rings) and is not AA.

**Type**

- UI: **Instrument Sans** (400/500/600). Code: **Geist Mono**. Both are OFL.
  - Ship them **bundled** in the app, never loaded from a font CDN (offline, privacy).
  - The prototype loads them from Google Fonts only because it's a web page.
- System fonts (SF Pro / SF Mono) are one setting away.
- Sizes:

  | Use | Size |
  |---|---|
  | UI (`text-ui`) | 13.5 px |
  | chat | 14.5 px, line height 1.65 |
  | code (`text-code`) | 12.5 px; 13 px in the editor |
  | labels | 12 px, weight 500, muted (never uppercase) |
  | section titles | 15 px, weight 600, −0.01 em |

**Shape and depth**

- Radius: 7 px for rows, 9–10 px for buttons and fields, 11 px for cards, 14–16 px for the approval card and composer, 12 px for windows.
- Shadows:
  - `--shadow-card`: a hairline ring plus a 1–3 px soft shadow;
  - `--shadow-overlay`: for the approval card and composer;
  - `--shadow-window`: popped-out windows.
- Borders are rare: hairlines (`--border`) inside cards only.

**Density and motion**

- Spacing is on a 4 px grid (`space-unit`).
- Rows are 30–34 px; touch-friendly where it matters (approval buttons 30–36 px).
- Motion is 90 ms for hover/press and 160 ms for panels. All of it is disabled under `prefers-reduced-motion` (as in M0-06).

**Icons:** lucide at 16 / 14 px, stroke 1.6, inheriting the text colour.

## 7. Draft theme files

- [`themes/atrium-calm-light.toml`](themes/atrium-calm-light.toml) and [`themes/atrium-calm-dark.toml`](themes/atrium-calm-dark.toml).
- They follow contract §4: `[meta]` / `[primitive]` / `[semantic]` / `[component]`, every required semantic token, and the same token names in light and dark.
- A Python check for this session verified:
  - the required set and reference resolution;
  - no semantic/component name collisions;
  - the AA ratios above.
- They have **not** been run through the M0-06 TypeScript loader; its dependencies aren't installed in this worktree. M1 should add them to the loader's CI check.
- `[component]` holds the layout tokens for B (widths, gaps, radii, measure), the overlay colours (hover, selected, bubble, state backgrounds), CR-01's shadows / scrim / glass blur, and the `syntax-*` colours.

## 8. What this implies for M1 (proposals, not decisions)

- **Token contract:** CR-02 (runtime opacity tokens and channel tokens for surfaces; translucent overlay and state-background tokens; folding CR-01 in).
- **Multi-window:**
  - Popped-out views need one daemon connection per window, or a shared client in the Rust core that forwards events to each `WebviewWindow`. The second is lighter and fits the "thin UI" rule in D-022.
  - Window state (which views are where, sizes, pinned) belongs in the per-workspace layout store PLAN §3.20 already describes.
- **Settings registry:** features declare `{ section, essentials[], advanced[] }`. The search index covers both lists.
- **Budgets:** layout B has no extra cost over M0-06. Cards and blur were measured there (Glass). Lazy-load the editor, terminal and diff views, as M0-06 §8 recommends.

## 9. Open questions (for Pedro or M1 briefs)

1. Fonts: keep Instrument Sans + Geist Mono, or default to the system fonts? Both are bundled either way.
2. Topic rules in §4: topics spanning workspaces, *Unsorted*, ⌃1…9.
3. Home: not designed in this round. Proposed: a chat card like any other, pinned as a tile.
4. Linux client-side decorations: the window controls on the right, the same frame. Not designed in this round.
5. Distribution: `macOSPrivateApi` (needed for opacity) rules out the Mac App Store. The orchestrator should record this when M1 starts.
6. The prototype's light state colours were darkened for AA after Pedro reviewed it, so the doc and themes are the reference. The published canvas still shows the earlier, slightly lighter values; republishing it was blocked by a permission check once the canvas was shared.
