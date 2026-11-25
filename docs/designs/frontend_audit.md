# Phase 0 — Complete Stitch Frontend Analysis & Backend Contract Audit

- **Project:** Thiruvalluvar AI — 100% Pure-Rust Bilingual Offline Edge System
- **Workspace Base:** `thiruvalluvar-ai-rs/` (sibling frontend archive directory: `../stitch files/`)
- **Target Viewports:** Laptop/Desktop (`1280×720` / `1366×768`) + Raspberry Pi 4B 3.5-Inch Landscape Touchscreen (`480×320`)
- **Runtime Mandate:** 100% Pure Rust • Zero Python • Zero Cloud APIs • Zero External CDNs • Verbatim SQLite Verse Integrity
- **Phase Roadmap Context:** Phase 0 (Frontend & DB Audit — *this document*) → Phase 1 (`docs/designs/backend_master_approved.md`) → Phase 2 (Embeddings + `user_state.db`) → Phase 3 (`kural-audio`) → Phase 4 (`kural-ai` + `kural-voice-cmd`) → Phase 5 (`kural-backend` + `kural-app`) → Phase 6 (Pi 4B Kiosk Deployment)
- **EGM Review Sign-Off:** Audited and resolved via `egm-comprehension`, `egm-codebase-details`, and `egm-structural`

---

## 0. Domain & Acronym Glossary

| Term / Acronym | Definition |
|---|---|
| **Paal (`பால்`)** | Major Division of the Thirukkural (`1`: Arathuppal / Virtue `1..=380`; `2`: Porutpal / Wealth & Polity `381..=1080`; `3`: Inbathuppal / Love & Affection `1081..=1330`). |
| **Iyal (`இயல்`)** | Classical Section grouping chapters within a Paal (`9` Iyals total across `133` chapters). |
| **Adhikaram (`அதிகாரம்`)** | Chapter of `10` couplets (`133` Adhikarams total = `1,330` Kurals). |
| **Kutty Story (`குட்டிக் கதை`)** | Short 4–6 sentence moral story tailored for Child Mode (Ages 6–12) illustrating a Kural. |
| **Scholars (`Scholar` enum)** | `MuVa` (`மு.வ.` — Dr. M. Varadarajanar), `Solomon` (`சாலமன்` — Solomon Pappiah), `Kalaignar` (`கலைஞர்` — M. Karunanidhi), `Parimel` (`பரிமேலழகர்` — medieval commentator Parimelazhagar, `1,328/1,330` present), and `English` (G. U. Pope translation & prose explanation). |
| **LPC / VTL** | **Linear Predictive Coding** (Burg method) used to estimate resonant vocal-tract formants ($F_1, F_2, F_3$) and physical **Vocal-Tract Length** ($L_{\text{vt}}$ in cm) so Child voices (`6–12`) are cleanly separated from Adult Female voices despite overlapping pitch ($F_0$). |
| **VAD / PTT** | **Voice Activity Detection** (`silero_vad.onnx` speech start/end detector) and **Push-To-Talk** (user taps mic button/orb to initiate a bounded voice command or question). |
| **RRF** | **Reciprocal Rank Fusion** — deterministic rank-merging formula $\sum \frac{1}{k + \text{rank}_i}$ combining SQLite FTS5 BM25 keyword search with `model2vec-rs` INT8 cosine similarity. |

---

## 1. Complete 16-Screen Stitch Inventory Matrix

Inside `../stitch files/`, all 16 Google Stitch exports exist as unpacked directories containing `code.html`. Each file has been copied cleanly into `stitch_extracted/<screen>_<lang>_<theme>.html` (sorted below in canonical `(ta/dark, ta/light, en/dark, en/light)` order per screen; note `wc -l` counts newline characters, and every file has +1 trailing line without newline).

| # | Source Directory (`../stitch files/`) | Extracted Target (`stitch_extracted/`) | Screen ID | `<html lang>` | `data-theme` | Newlines | Interactive DOM Count | Hardcoded Mock Kurals |
|---:|---|---|---|---|---|---:|---|---|
| 1 | `home_stitch_thiruvalluvar_ai_kiosk_app(6)` | `home_ta_dark.html` | `S1` Start / Home | `ta` | `dark` | 907 | `4` `<button>` + `3` `role="button"` cards | None |
| 2 | `home_stitch_thiruvalluvar_ai_kiosk_app(7)` | `home_ta_light.html` | `S1` Start / Home | `ta` | `light` | 920 | `4` `<button>` + `3` `role="button"` cards | None |
| 3 | `home_stitch_thiruvalluvar_ai_kiosk_app(4)` | `home_en_dark.html` | `S1` Start / Home | `en` | `dark` | 920 | `4` `<button>` + `3` `role="button"` cards | None |
| 4 | `home_stitch_thiruvalluvar_ai_kiosk_app(5)` | `home_en_light.html` | `S1` Start / Home | `en` | `light` | 920 | `4` `<button>` + `3` `role="button"` cards | None |
| 5 | `child_stitch_thiruvalluvar_ai_kiosk_app(1)` | `child_ta_dark.html` | `S2` Child 3D (`6–12`) | `ta` | `dark` | 894 | `19` `<button>` + `1` `<a>` back link | `#1`, `#391`, `#100` |
| 6 | `child_stitch_thiruvalluvar_ai_kiosk_app` | `child_ta_light.html` | `S2` Child 3D (`6–12`) | `ta` | `light` | 1038 | `19` `<button>` + `1` `<a>` back link | `#1`, `#391`, `#100` |
| 7 | `child_stitch_thiruvalluvar_ai_kiosk_app(3)` | `child_en_dark.html` | `S2` Child 3D (`6–12`) | `en` | `dark` | 743 | `19` `<button>` + `1` `<a>` back link | `#1`, `#391`, `#100` |
| 8 | `child_stitch_thiruvalluvar_ai_kiosk_app(2)` | `child_en_light.html` | `S2` Child 3D (`6–12`) | `en` | `light` | 745 | `19` `<button>` + `1` `<a>` back link | `#1`, `#391`, `#100` |
| 9 | `youth_stitch_thiruvalluvar_ai_kiosk_app(3)` | `youth_ta_dark.html` | `S3` Youth Royal 3D (`13–17`) | `ta` | `dark` | 935 | `24` `<button>` + `1` `<a>` back link | `#481`, `#391`, `#619` |
| 10 | `youth_stitch_thiruvalluvar_ai_kiosk_app(8)` | `youth_ta_light.html` | `S3` Youth Royal 3D (`13–17`) | `ta` | `light` | 698 | `24` `<button>` + `1` `<a>` back link | `#481`, `#391`, `#619` |
| 11 | `youth_stitch_thiruvalluvar_ai_kiosk_app(7)` | `youth_en_dark.html` | `S3` Youth Royal 3D (`13–17`) | `en` | `dark` | 696 | `24` `<button>` + `1` `<a>` back link | `#481`, `#391`, `#619` |
| 12 | `youth_stitch_thiruvalluvar_ai_kiosk_app(9)` | `youth_en_light.html` | `S3` Youth Royal 3D (`13–17`) | `en` | `light` | 695 | `24` `<button>` + `1` `<a>` back link | `#481`, `#391`, `#619` |
| 13 | `adult_stitch_thiruvalluvar_ai_kiosk_app(1)` | `adult_ta_dark.html` | `S4` Adult Royal 3D (`18+`) | `ta` | `dark` | 854 | `29` `<button>` + `1` `<a>` + `1` `onclick` `<span>` (`#pi-speed-chip`) | `#423`, `#355`, `#1` |
| 14 | `adult_stitch_thiruvalluvar_ai_kiosk_app(4)` | `adult_ta_light.html` | `S4` Adult Royal 3D (`18+`) | `ta` | `light` | 717 | `29` `<button>` + `1` `<a>` + `1` `onclick` `<span>` (`#pi-speed-chip`) | `#423`, `#355`, `#1` |
| 15 | `adult_stitch_thiruvalluvar_ai_kiosk_app(2)` | `adult_en_dark.html` | `S4` Adult Royal 3D (`18+`) | `en` | `dark` | 721 | `29` `<button>` + `1` `<a>` + `1` `onclick` `<span>` (`#pi-speed-chip`) | `#423`, `#355`, `#1` |
| 16 | `adult_stitch_thiruvalluvar_ai_kiosk_app(3)` | `adult_en_light.html` | `S4` Adult Royal 3D (`18+`) | `en` | `light` | 719 | `29` `<button>` + `1` `<a>` + `1` `onclick` `<span>` (`#pi-speed-chip`) | `#423`, `#355`, `#1` |

---

## 2. Extracted Design System & Visual Language Tokens

### 2.1 Shared Core Color Tokens (`tailwind.config`)

| Token Group | Light Hex (`on-*` Foreground) | Dark Hex (`on-*` Foreground) | Functional Role |
|---|---|---|---|
| `gold-frame` | `#E6C068` | `#E6C068` | Royal borders, focus rings, emblem accents |
| `gradient-start` / `mid` / `end` | `#FFB300` → `#FF6A00` → `#FF2D55` | `#FFB300` → `#FF6A00` → `#FF2D55` | Mic Orb, primary Listen CTA, 5-bar voice equalizer |
| `*-bg` | `#FFF8EC` (`#2B1B0E`) | `#0B1412` (`#F2F5F4`) | Root parchment (Light) & obsidian-forest (Dark) canvas |
| `*-surface` / `*-surface-2` | `#FFFFFF` / `#FBEFD9` (`#2B1B0E`) | `#101A2B` / `#16233A` (`#F2F5F4`) | Card & secondary pill surfaces |
| `*-text-muted` / `*-outline` | `#6B5A48` / `#D9C7AD` | `#9AA9A6` / `#25354D` | Secondary captions & structural borders |
| `*-primary` | `#D84315` (`#080808`) | `#FFB300` (`#1A1100`) | Active Kural highlights & primary interactive badges |
| `*-child-sky` | `#81D4FA` (`#0B1412`) | `#4FC3F7` (`#0B1412`) | Child (`6–12`) playful sky accents |
| `*-teen` | `#00796B` (`#FFFFFF`) | `#19C9A0` (`#06241C`) | Youth (`13–17`) emerald-teal accents |
| `*-adult-surface` | `#FFF3DC` (`#8E1B1B`) | `#2A0F10` (`#E6C068`) | Adult (`18+`) manuscript & imperial burgundy surfaces |
| `*-success` | `#1B8F5A` (`#131313`) | `#2ECC8F` (`#06241C`) | Offline status dot, confidence meter, Kids Story card |
| `*-warning` / `*-error` | `#B26A00` / `#C62828` | `#FFC107` / `#FF5470` | Low-confidence retry & validation states |

### 2.2 Age-Specific 3D Surface & Motion Treatments

1. **`S1` Start Screen**: Minimalist heritage line art (`.pi-corner-art`), gold-framed portrait (`#valluvar-portrait`), pulsing mic halo (`.mic-pulse-ring`, `1.8s`), 3-column interactive age cards.
2. **`S2` Child 3D Playground (Ages 6–12)**:
   - Tactile 3D claymorphic cards (`.clay-card`, bottom lip `border-b-[6px]`) and chunky pressable buttons (`.toy-btn` with `translateY(4px)` press physics).
   - Pure-SVG animated sky backdrop: `.scene-light` (rotating `#sunGrad` sun + drifting clouds) and `.scene-dark` (`#moonGrad` crescent moon + twinkling stars + 4 glowing `#FFE066`/`#80FFDB` fireflies).
3. **`S3` Youth Royal 3D (Ages 13–17)**:
   - Chola `#cholaLattice` 56×56 diamond-circle SVG watermark + drifting gold dust motes (`.anim-mote`, S3 only).
   - Sculpted gold-beveled cards (`.royal-card-primary`: dark `#2A0F11`→`#230B0D`→`#1A080A`, `2.5px solid #C8A152`, `0 6px 0 #7D5D22` bevel; `.royal-card-secondary`: `#142138`→`#10192B`).
4. **`S4` Adult Scholar Royal 3D (Ages 18+)**:
   - Chola `#cholaFiligree` 64×64 SVG watermark (no `.anim-mote` particles, keeping scholarly reading distraction-free).
   - Deep Imperial Burgundy manuscript cards (`#270C0E`→`#1C080A`, `0 6px 0 #6E511D` bevel), `.royal-subcard` sub-panels, and `.royal-seal` (`#kural-seal-wrap` / `#kural-initial-seal`): 56×56 3D crimson-and-gold embossed tile displaying `opening_letter_ta`.

### 2.3 Typography, Dual-Viewport Budgets & Zero-Scroll Modal Overlay Spec

- **Font Stack**:
  - `font-kural`: `'Noto Serif Tamil', sans-serif` (`line-height: 1.6`)
  - `html[lang="ta"] body`: `'Noto Sans Tamil', 'Inter', sans-serif` (`line-height: 1.45–1.5` to prevent Tamil vowel-sign clipping)
  - `font-child`: `'Baloo Thambi 2', sans-serif` (loaded on S1/S2; bundled workspace-wide in Phase 5)
  - `font-display` / `headline` / `body` / `label`: `'Inter', sans-serif`
- **Viewport Budgets**:
  - **Desktop (`1280×720` / `1366×768`)**: `64px` header + `flex-1` 12-col grid (`5+7` on S1, `7+5` on S2, `6+6` on S3/S4) + `56px` (`48px` on S1) footer.
  - **Pi Touchscreen (`480×320`, `@media (max-width: 560px) and (max-height: 380px)`)**:
    - `S1`: `40px` header + `1fr` (`5fr 7fr` split) + `82px` 3-col manual bar = **`<= 320px`**.
    - `S2`: `38px` header + `184px` Kural card + `90px` **5-slot dock** (`Story`, **`Game`**, `68px` `#pi-voice-orb`, `Audio`, `Search` — expanded from Stitch's 3-slot dock so `Game` & `Search` have first-class `480×320` touch targets) = **`312px <= 320px`**.
    - `S3`: `38px` header + `192px` (`26px` **3-tab** `#pi-mode-strip`: `குறள்` | `பகுப்பாய்வு` | `சவால்/Quiz` + Kural/Insight card) + `84px` 5-slot dock (`Paals`, `Deep`, `64px` `#pi-voice-orb`, `Search`, `AI`) = **`314px <= 320px`**.
    - `S4`: `38px` header + `192px` (`26px` 5-tab `#pi-mode-strip`: `குறள்` | `மு.வ.` | `சாலமன்` | `கலைஞர்` | `ஆய்வு` + Kural/Study card) + `84px` 5-slot dock (`Paals`, `Scholars`, `64px` `#pi-voice-orb`, `Search`, `Discuss`) = **`314px <= 320px`**.
  - **6 Zero-Scroll Modal Overlays (`ModalKind::{Search, AiChat, KidsStory, KidsWordGame, YouthQuiz, YouthDeepDive}`)**:
    - Render as `position: fixed; inset: 0; z-index: 40; height: 100dvh; width: 100vw;` backdrop + bounded card (`max-height: 304px` on `480×320`, `max-height: 640px` on `1280×720`) with a persistent `36px` modal header + top-right `✕` (`BackendCommand::CloseModal`) and internal paginated/clamped content area so the outer shell never scrolls.

---

## 3. Complete Interactive Element Catalog & Typed Backend Contract

All 5 sub-tables share a uniform 6-column schema: `Selector / Element`, `Viewport` (`Both` | `Desktop` | `Pi`), `DOM Status` (`Present` | `Passive` | `Missing`), `UI Trigger`, `BackendCommand`, and `BackendEvent / State Update`.

### 3.1 Global Controls (`S1`–`S4`)

| Selector / Element | Viewport | DOM Status | UI Trigger | `BackendCommand` | `BackendEvent / State Update` |
|---|---|---|---|---|---|
| `#theme-toggle` (`#icon-sun`, `#icon-moon`) | `Both` | `Present` | Tap theme icon | `SetTheme(Theme)` (`Dark` / `Light`) | `PrefsUpdated(UserPrefs)` — persists in `user_state.db`, toggles `.dark` & `data-theme` |
| `#lang-toggle` (`#lang-ta[-pill]`, `#lang-en[-pill]`) | `Both` | `Present` | Tap `தமிழ்` / `EN` pill | `SetLanguage(Language)` (`Tamil` / `English`) | `PrefsUpdated(UserPrefs)` — switches UI copy, scholar labels & STT/TTS language |
| `a[aria-label="Home"]` (`←`) | `Both` (`S2`–`S4`) | `Present` | Tap back button | `Navigate(AppScreen::Start)` | `ScreenChanged(AppScreen::Start)` (replaces broken `<a href>`) |
| `nav[aria-label="Age Group"] button[0..2]` | `Desktop` (`S2`–`S4`) | `Present` | Tap `Child` / `Youth` / `Adult` | `SelectAge(AgeGroup)` | `ScreenChanged(AppScreen)` + `KuralLoaded(FullKural)` |
| `#voice-hud`, `.eq-bar` (×5), `#voice-hud-text`, close `✕` | `Both` (`S2`–`S4`) | `Present` | Close `✕` button | `CancelVoice` | Driven by `MicRms(f32)` (30 Hz), `AudioStateChanged(AudioState)`, `VoiceToast { text_ta, text_en }` |

### 3.2 Screen `S1` — Start / Home Screen (`home_*`)

| Selector / Element | Viewport | DOM Status | UI Trigger | `BackendCommand` | `BackendEvent / State Update` |
|---|---|---|---|---|---|
| `#mic-btn` (`.pi-mic-btn`) | `Both` | `Present` | Tap glowing mic orb (PTT) | `StartVoiceAgeDetect` | `AudioStateChanged(Listening)` → `AgeDetected { age, confidence, low_confidence, f0_hz, vtl_cm, latency_ms }` or `AgeRetry(RetryReason)` |
| `#detect-result` + `button[data-i18n="enter"]` | `Both` | `Present` | Tap `Continue` after detection | `SelectAge(detected_age)` | `ScreenChanged(AppScreen)` + `KuralLoaded(FullKural)` (low confidence `< 0.70` highlights manual cards) |
| `.pi-age-grid > div[role="button"]` (×3) | `Both` | `Present` | Tap `Child` / `Teen` / `Adult` card | `SelectAge(AgeGroup::{Kids, Youth, Adult})` | `ScreenChanged(AppScreen)` + `KuralLoaded(FullKural)` |
| `.pi-inline-offline` / `[data-component-id="badge-status"]` | `Both` | `Passive` | Read-only status dot/pill | None *(fix missing footer in `home_ta_dark`)* | Reflects `SystemHealth { db_ok, audio_ok, models_ok }` |

### 3.3 Screen `S2` — Child 3D Animated Screen (`child_*`, Ages 6–12)

| Selector / Element | Viewport | DOM Status | UI Trigger | `BackendCommand` | `BackendEvent / State Update` |
|---|---|---|---|---|---|
| `.pi-star-pill` (`⭐`) | `Both` | `Passive` | Read-only star counter badge | None | `KidsProgressUpdated { stars_earned, completed_stories, completed_games }` |
| `button[onclick="triggerAction('search')"]` & Pi 5th dock tile | `Both` | `Present` (`Desktop`), `Missing` (`Pi`) | Tap `🔍 தேடு / Search` | `OpenModal(ModalKind::Search)` | `ModalChanged(Some(ModalKind::Search))` |
| `#prev-kural` (`◀`) / `#next-kural` (`▶`) | `Both` | `Present` | Tap arrow buttons | `PrevKural` / `NextKural` | `KuralLoaded(FullKural)` (steps active filter list, or `kids_featured == true` pool `1..=1080` when unfiltered) |
| `#btn-listen` (`🔊 குறளைக் கேள்`) | `Both` | `Present` | Tap Listen | `SpeakCurrent(SpeakTarget::KuralAndMeaning)` | `AudioStateChanged(PlayingTts)` — recites `tts_kural_*` + `simple_meaning_*` |
| `button` (`மீண்டும் / Replay`, line 771) | `Both` | `Present` | Tap Replay | `ReplayAudio` | `AudioStateChanged(PlayingTts)` — zero-latency PCM buffer replay |
| `Stop` button inside `#kural-actions` (when playing) | `Both` | `Missing` | Tap `⏹ நிறுத்து / Stop` | `StopAudio` | `AudioStateChanged(Idle)` (`< 20 ms` sink abort) |
| `button` (`தாத்தாவிடம் பேசு / Talk to Grandpa`, line 775) | `Both` | `Present` | Tap chat button | `OpenModal(ModalKind::AiChat)` | `ModalChanged(Some(ModalKind::AiChat))` (Kids Grandpa persona, supports text + PTT voice) |
| `button[onclick="triggerAction('story')"]` (Desktop card & Pi dock) | `Both` | `Present` | Tap `குறள் கதைகள் / Story` | `OpenModal(ModalKind::KidsStory)` | `ModalChanged(Some(ModalKind::KidsStory))` — displays `story_title_*`, `kutty_story_*`, `moral_*` + `SpeakTarget::KuttyStory` |
| `button[onclick="triggerAction('audio')"]` (Desktop card & Pi dock) | `Both` | `Present` | Tap `கேட்டு மகிழ்! / Listen` | `SpeakCurrent(SpeakTarget::KuttyStory)` | `AudioStateChanged(PlayingTts)` — narrates full Kutty Story + moral |
| `Games` (`🎮 விளையாட்டு / Word Game`) card & Pi dock tile | `Both` | `Missing` *(orphaned in JS)* | Tap `விளையாட்டு / Game` | `OpenModal(ModalKind::KidsWordGame)` → `SubmitKidsGameAnswer { kural_number: u16, choice_idx: u8 }` | `KidsGameEvaluated { correct: bool, correct_idx: u8, stars_earned: u32 }` |
| `#desktop-voice-btn` / `#pi-voice-orb` | `Both` | `Present` | Tap mic button/orb (PTT) | `StartVoiceCommand` | `AudioStateChanged(Listening)` → `VoiceCommandRecognized(BackendCommand)` |
| `#sound-toggle` (`#sound-icon`, `#sound-label`) | `Desktop` | `Present` | Tap sound pill | `SetMuted(bool)` | `PrefsUpdated(UserPrefs)` |

> **Kids Word Game Bilingual Validation Rule**: `kural_child` stores `game_blank_word_ta` inside `game_options_ta[4]` at deterministic slot `s`, and places the matching English meaning at the **same index `s`** in `game_options_en[4]`. In both languages, the modal displays the Tamil couplet with `game_blank_word_ta` masked as `_____` (plus `simple_meaning_en` hint in English mode) and 4 option buttons (`0..=3` showing `game_options_ta[i]` paired with `game_options_en[i]`). `SubmitKidsGameAnswer { kural_number, choice_idx }` computes `correct_idx = game_options_ta.iter().position(|w| w == &game_blank_word_ta)` and checks `usize::from(choice_idx) == correct_idx`.

### 3.4 Screen `S3` — Youth Royal 3D Screen (`youth_*`, Ages 13–17)

| Selector / Element | Viewport | DOM Status | UI Trigger | `BackendCommand` | `BackendEvent / State Update` |
|---|---|---|---|---|---|
| `.pi-streak-pill` (`🔥`) | `Both` *(unhide on Desktop)* | `Passive` | Read-only streak & XP badge | None | `YouthProgressUpdated { daily_streak, quiz_xp }` |
| `#pi-mode-strip` (`குறள்` \| `பகுப்பாய்வு` \| `சவால்`) | `Pi` | `Present` (2 tabs) + `Missing` 3rd `Quiz` tab | Tap compact mode tab | `SetCompactTab(YouthTab)` or `OpenModal(ModalKind::YouthQuiz)` | Updates local Pi panel or emits `ModalChanged(Some(ModalKind::YouthQuiz))` |
| `#prev-kural` (`◀`) / `#next-kural` (`▶`) | `Both` | `Present` | Tap arrow buttons | `PrevKural` / `NextKural` | `KuralLoaded(FullKural)` (steps active filtered list or `1..=1330`) |
| `button[onclick="triggerListen()"]` (Listen & Replay, lines 641, 645) | `Both` | `Present` | Tap `கேள்` / `மீண்டும்` | `SpeakCurrent(SpeakTarget::YouthBriefing)` / `ReplayAudio` | `AudioStateChanged(PlayingTts)` |
| `button[onclick="stopAudio()"]` (line 649) | `Both` *(unhide on Desktop)* | `Present` | Tap `நிறுத்து / Stop` | `StopAudio` | `AudioStateChanged(Idle)` |
| `button[onclick="triggerToast('deep')"]` (`விரிவான உரை`, `மேலும் →`, Pi `Deep`) | `Both` | `Present` | Tap Deep Dive | `OpenModal(ModalKind::YouthDeepDive)` | `ModalChanged(Some(ModalKind::YouthDeepDive))` |
| `button[onclick="toggleVoiceHud()"]` (`AI உரையாடல்`, Pi `AI` tile) | `Both` | `Present` | Tap Ask AI | `OpenModal(ModalKind::AiChat)` | `ModalChanged(Some(ModalKind::AiChat))` (Youth Mentor persona, seeded with active Kural) |
| Topic Tag Chips & Daily Quiz CTA (orphaned `selectTag`, `#tag-search`, `msgs.quiz`) | `Both` | `Missing` *(orphaned in JS)* | Tap tag chip or `Daily Quiz` button | `FilterByTag(Option<String>)` / `OpenModal(ModalKind::YouthQuiz)` → `SubmitYouthQuizAnswer { kural_number: u16, choice_idx: u8 }` | `FilterUpdated { label, count }` + `KuralLoaded(FullKural)` / `YouthQuizEvaluated { correct, correct_idx, daily_streak, quiz_xp }` |
| `button[onclick="triggerToast('search')"]` & Pi `Paals` (`triggerToast('paals')`) | `Both` | `Present` | Tap `Quick Search` / `Search` / `Paals` | `OpenModal(ModalKind::Search)` (`Paals` opens Search on Divisions tab) | `ModalChanged(Some(ModalKind::Search))` |
| `button[onclick="cycleSpeed()"]` (`#dock-speed-label`) + mute button | `Both` | `Present` (`Desktop`), `Missing` (`Pi` + mute) | Tap speed pill *(standardize `[0.75, 1.0, 1.25]`)* | `SetSpeechSpeed(f32)` / `SetMuted(bool)` | `PrefsUpdated(UserPrefs)` *(note: Stitch S3 had `['1.0x', '1.25x', '0.85x']` & orphaned `#card-speed-btn`)* |
| Desktop Voice pill (line 701) / `#pi-voice-orb` (line 729) | `Both` | `Present` | Tap mic pill/orb (PTT) | `StartVoiceCommand` | `AudioStateChanged(Listening)` → `VoiceCommandRecognized(BackendCommand)` |

### 3.5 Screen `S4` — Adult / Scholars Royal 3D Screen (`adult_*`, Ages 18+)

| Selector / Element | Viewport | DOM Status | UI Trigger | `BackendCommand` | `BackendEvent / State Update` |
|---|---|---|---|---|---|
| `#pi-mode-strip` (`குறள்` \| `மு.வ.` \| `சாலமன்` \| `கலைஞர்` \| `ஆய்வு`) | `Pi` | `Present` | Tap 1 of 5 compact tabs | Tab 1: Verse view; Tabs 2–4: `SelectCommentary(Scholar)`; Tab 5 (`ஆய்வு`): Study/Themes/Bookmark panel | `CommentarySelected(Scholar)` or switches local Pi panel |
| `#kural-initial-seal` (`#kural-seal-wrap`) | `Desktop` | `Passive` | Embossed 3D initial letter | None | Bound to `KuralCore.opening_letter_ta` (always Tamil grapheme, never Latin) |
| `#pi-speed-chip` + Desktop `.speed-pill` (`0.75x`/`1.0x`/`1.25x`) + mute | `Both` | `Present` (`Pi` `<span>`), `Missing` (`Desktop` & mute) | Tap speed `<button>` | `SetSpeechSpeed(f32)` / `SetMuted(bool)` | `PrefsUpdated(UserPrefs)` *(convert `#pi-speed-chip` `<span>` to `<button>`)* |
| `#prev-kural` (`◀`) / `#next-kural` (`▶`) | `Both` | `Present` | Tap arrow buttons | `PrevKural` / `NextKural` | `KuralLoaded(FullKural)` |
| `.comm-tab[data-comm]` (`மு.வ.`, `சாலமன்`, `கலைஞர்`, `English`, + `பரிமேலழகர்`) & Pi `Scholars` tile | `Both` | `Present` (4 tabs) + `Missing` (`Parimel` 5th tab) | Tap scholar tab (Pi `Scholars` cycles scholars) | `SelectCommentary(Scholar)` | `CommentarySelected(Scholar)` *(for Kurals `#395` & `#648` where `meaning_parimel` is `None`, disable `Parimel` tab with bilingual tooltip and fall back to `MuVa`)* |
| `Listen` (line 548), `Replay` (line 552), `Stop` (line 556) | `Both` | `Present` | Tap audio buttons | `SpeakCurrent(SpeakTarget::ScholarCommentary(scholar))`, `ReplayAudio`, `StopAudio` | `AudioStateChanged(PlayingTts \| Idle)` |
| `Discuss` (`விவாதி`, line 560; `Discuss →`, line 579; Pi `Discuss`, line 630) | `Both` | `Present` | Tap Discuss | `OpenModal(ModalKind::AiChat)` (pre-seeded with active `kural.number`) | `ModalChanged(Some(ModalKind::AiChat))` → user submits `AskAiQuestion { query: String, seed_kural: Some(u16) }` → `AiAnswerReady(GroundedAnswer)` |
| Research Theme Subcards (`.royal-subcard` ×4) & Bookmark Button (in truncated Desktop right col + Pi `ஆய்வு` tab) | `Both` | `Missing` *(orphaned `selectThemeKural` & `.royal-subcard`)* | Tap theme filter or `★ Bookmark` | `FilterByResearchTheme(Option<ResearchTheme>)` / `ToggleBookmark(u16)` | `FilterUpdated` + `KuralLoaded(FullKural)` / `BookmarksUpdated(Vec<BookmarkEntry>)` (saves active `Scholar` automatically) |
| `Search Kural / Adhikaram` (line 587) & Pi `Paals` / `Search` tiles | `Both` | `Present` | Tap Search or Paals | `OpenModal(ModalKind::Search)` | `ModalChanged(Some(ModalKind::Search))` |
| Desktop Voice pill (line 592) / `#pi-voice-orb` (line 619) | `Both` | `Present` | Tap mic pill/orb (PTT) | `StartVoiceCommand` | `AudioStateChanged(Listening)` → `VoiceCommandRecognized(BackendCommand)` |

### 3.6 Shared Behavioral Rules (Filters, Speed & Speech Targets)
- **Filter Lifecycle**: `FilterByDivision(Option<u8>)`, `FilterByTag(Option<String>)`, and `FilterByResearchTheme(Option<ResearchTheme>)` set an active ordered `Vec<u16>` Kural list in `BackendController` (with a visible `"✕ Clear Filter"` chip); `PrevKural` and `NextKural` step within that active list and revert to the default pool (`1..=1080` `kids_featured` on `S2`, `1..=1330` on `S3`/`S4`) when cleared.
- **Speech Speed Precedence**: `preferences.speech_speed` stores a user multiplier (`0.75`, `1.0`, or `1.25`, default `1.0`), multiplied by a gentle persona cadence factor (`0.95` Kids, `1.02` Youth, `1.00` Adult) so `SelectAge` never overwrites the user's chosen speed pill.
- **`SpeakTarget` Enum**: `SpeakTarget::{KuralOnly, KuralAndMeaning, KuttyStory, YouthBriefing, ScholarCommentary(Scholar), AiAnswer}`.

---

## 4. Complete Frontend-to-Database Data Contract Map

### 4.1 Read-Only Literary & Enrichment Store (`data/thirukkural.db`)

| UI Element / Modal | Exact `kural-core` / `kural-db` Fields | SQLite / FTS5 Table(s) | Status in `data/thirukkural.db` |
|---|---|---|---|
| `#kural-badge-text`, `#kural-div-tag`, 3-Paal / 9-Iyal / 133-Adhikaram Search Browser | `KuralCore.number`, `Division.{id, name_ta, alt_name_ta, name_en, transliteration, kural_start, kural_end}`, `Iyal.{id, name_ta, alt_name_ta, name_en, chapter_start, chapter_end}`, `Adhikaram.{id, name_ta, name_en, transliteration, kural_start, kural_end}` | `kurals`, `divisions`, `iyals`, `adhikarams`, `db_meta` | **READY** (`1,330` / `3` / `9` / `133`) |
| `#kural-initial-seal` (`S4`), `#kural-couplet` (`S2`–`S4`), `#kural-translit` (`S3`–`S4`) | `opening_letter_ta`, `line1_ta`, `line2_ta` *(always Tamil in `#kural-couplet`)*, `transliteration1`, `transliteration2`, `couplet_en`, `tts_kural_ta`, `tts_kural_en` | `kurals` | **READY** (`1,330` NFC & phonetic-audited) |
| `S2` `#kural-meaning`, `KidsStory` Modal, `KidsWordGame` Modal | `kids_featured` (`INTEGER 0/1`), `simple_meaning_ta`/`en`, `story_title_ta`/`en`, `kutty_story_ta`/`en`, `moral_ta`/`en`, `game_blank_word_ta`, `game_options_ta` (JSON `TEXT[4]`), `game_options_en` (JSON `TEXT[4]`) | `kural_child` | **READY** (`1,330` rows: `1,080` featured, `250` child-safe Inbam) |
| `S3` `#kural-meaning`, Right-Col Analytics, `YouthDeepDive` & `YouthQuiz` Modals, Tag Chips | `KuralCore.{meaning_solomon, translation_en}` (for `#kural-meaning`), `KuralYouth.{analysis_ta, analysis_en, action_tip_ta, action_tip_en, quiz_question_ta, quiz_question_en, quiz_options_ta, quiz_options_en, quiz_correct_idx}`, `Tag.{id, slug, name_ta, name_en}` | `kurals`, `kural_youth`, `tags`, `kural_tags` | **READY** (`1,330` rows + `4,856` tag links across `20` tags) |
| `S4` `#comm-body`, `#pi-comm-text`, Daily Wisdom Reflection, Research Theme Subcards | `KuralCore.{meaning_mu_va, meaning_solomon, meaning_kalaignar, meaning_parimel, explanation_en, couplet_en}`, `KuralAdult.{reflection_ta, reflection_en, research_theme}` | `kurals`, `kural_adult` | **READY** (`1,330` rows; `meaning_parimel` in `1,328/1,330`, `NULL` only on `#395` & `#648`) |
| `Search` Modal & `AiChat` Modal (`GroundedAnswer { answer_ta, answer_en, cited_kurals: Vec<FullKural>, out_of_scope: bool, latency_ms }`) | `SearchOutcome` (`ExactNumber`, `AdhikaramMatch`, `FullText`, `FuzzyFallback`, `did_you_mean`), `KnowledgeEntry.{id, category, question_ta, question_en, answer_ta, answer_en, keywords}` | `kurals_fts` (`1,330`), `knowledge_base` (`45`), `knowledge_fts` (`45`), + *Phase 2 `*_embeddings`* | **FTS5 + Fuzzy READY**; **INT8 Embeddings in Phase 2** |

### 4.2 Mutable User State Store (`data/user_state.db` — Phase 2 Addition)

| Consuming UI Element / Modal | `UserStateStore` Struct / Fields | SQLite Table (`data/user_state.db`) | Mutating `BackendCommand` |
|---|---|---|---|
| `#theme-toggle`, `#lang-toggle`, Speed Chips, `#sound-toggle`, last viewed Kural per age | `UserPrefs { lang, theme, speech_speed, volume, muted, default_age, last_kural_kids, last_kural_youth, last_kural_adult }` | `preferences` | `SetLanguage`, `SetTheme`, `SetSpeechSpeed`, `SetMuted`, `AdjustVolume`, `OpenKural` |
| `S2` `.pi-star-pill` (`⭐`), `KidsStory` & `KidsWordGame` completion badges | `KidsProgress { stars_earned: u32, completed_stories: BTreeSet<u16>, completed_games: BTreeSet<u16> }` | `kids_progress` | `OpenModal(KidsStory)`, `SubmitKidsGameAnswer` |
| `S3` `.pi-streak-pill` (`🔥`), `YouthQuiz` XP counter | `YouthProgress { daily_streak: u32, last_active_date: String, quiz_xp: u32, answered_quizzes: BTreeSet<u16> }` | `youth_progress` | `SubmitYouthQuizAnswer` |
| `S4` Bookmark star toggle & Saved Bookmarks subcard | `BookmarkEntry { kural_number: u16, preferred_scholar: Scholar, created_at: String }` | `adult_bookmarks` | `ToggleBookmark(u16)` (captures active `Scholar` from controller state) |

### 4.3 `Synopsis.pdf` Literary & Safety Mandates

*(No standalone `Synopsis.pdf` binary exists in the workspace directory; the master prompt's `Synopsis.pdf` specification requirements are enforced verbatim below)*:
1. **100% Verbatim Verse Integrity**: `line1_ta`, `line2_ta`, Kural numbers (`1..=1330`), Adhikaram names (`1..=133`), and division boundaries (`Aram 1–380`, `Porul 381–1080`, `Inbam 1081–1330`) are ALWAYS queried verbatim from `KuralDb`. Unlike Stitch's `*_en_*.html` mockups—which overwrote `#kural-couplet` with English text when toggled to `EN`—`kural-app` keeps the sacred Tamil couplet visible in `#kural-couplet` across both languages while rendering English translation/couplet beneath it.
2. **Explicit Out-of-Scope Limitation Guardrail**: When combined FTS5 + `model2vec` RRF score is below threshold, `SemanticExplainer` returns `out_of_scope: true` and `cited_kurals: vec![]` with a clear, polite message in the active language (`ta` or `en`).
3. **Dual Modality & Motion Accessibility**: Every action is reachable via Touch/Click and Bilingual Voice (`ta` & `en`), respecting `@media (prefers-reduced-motion: reduce)`.

---

## 5. Offline Asset Violation Audit (Must Be Replaced in Phase 5)

| # | External URL Pattern in Stitch HTML | Occurrences Across 16 Files | Offline Failure Mode | Phase 5 Bundled Replacement |
|---:|---|---|---|---|
| 1 | `https://cdn.tailwindcss.com` (+ `?plugins=forms,container-queries` on `S1`) | **16/16** files (`16` `<script>` tags) | Complete layout collapse offline | Pre-compile static stylesheet `crates/kural-app/assets/app.css` |
| 2 | `https://fonts.googleapis.com/css2?...` & `https://fonts.gstatic.com` (`Inter`, `Noto Sans Tamil`, `Noto Serif Tamil`, `Baloo Thambi 2`) | **16/16** files (`S2`–`S4` also have `fonts.gstatic.com` preconnect) | Missing Tamil/display fonts or blocked render | Bundle local `.woff2` fonts in `crates/kural-app/assets/fonts/` with `@font-face` |
| 3 | `https://lh3.googleusercontent.com/aida/ALJJyrCW...` (Thiruvalluvar portrait) | **8/16** files, **12 `<img>` tags** (`1` `#valluvar-portrait` per `home_*`; `2` unnamed avatars per `child_*`) | Broken `<img>` icons offline | Bundle `crates/kural-app/assets/valluvar-portrait.png` |

*(Note: `http://www.w3.org/2000/svg` in inline `<svg>` elements is an XML namespace string, not a network fetch).*

---

## 6. Responsive & Frontend Defect Audit (Must Be Fixed in Phase 5)

| # | Severity | Defect Summary | File & Line Evidence | Impact | Phase 5 Fix |
|---:|---|---|---|---|---|
| 1 | **Critical** | `height: 1024px` inside `480×320` media query | `home_*` L392 (`.pi-root { height: 1024px !important; }`); `child_*` L484/L418/L314, `youth_*` L442/L294, `adult_*` L384/L327 (`#app-shell { height: 1024px; }`) — Stitch overwrote `100dvh` preserved in `data-stitch-css-vh` | Pushes Zone 2 & Zone 3 dock `704px` off-screen on `480×320` Pi LCD | Set `.pi-root, #app-shell { height: 100dvh !important; max-height: 100dvh; width: 100vw; overflow: hidden; }` |
| 2 | **High** | Missing `<footer class="pi-footer">` in `home_ta_dark.html` & unlocalized static `home_ta_light.html` | `home_ta_dark.html` L761–L763 (907 lines vs 920); `home_ta_light.html` L607–L774 ships English static DOM & `#icon-sun` visible | Breaks Desktop `justify-between` vertical centering & flashes wrong language/icon | Render unified `start_screen.rs` Dioxus component driven directly by `Language` & `Theme` state |
| 3 | **High** | Truncated `S4` Desktop right column (`~75%` blank) & stripped header pills | `adult_*` L454 (`<!-- Center: 4 Scholarly Links -->` empty), L569–L582 (`col-span-6` has only `shrink-0` reflection strip; `.royal-subcard` CSS unused); `youth_*` L580 empty | Wastes right pane on `1280×720` and hides Research Themes, Tags, Quiz & Bookmarks | Populate `S4` right column with Reflection + 4 `.royal-subcard` Research Theme cards + Bookmarks; populate `S3` with Analytics + Tag Chips + Daily Quiz CTA |
| 4 | **High** | Cross-variant language-toggle & scholar-tab inconsistencies | `*_en_*.html` (`S2`–`S4`) overwrite `#kural-couplet` with English and `adult_en_*` overwrites `#kural-initial-seal` with Latin; `adult_*` scholar tabs lack `data-ta`/`data-en` | Violates verbatim Tamil couplet display & leaves scholar tab labels unlocalized | Keep `#kural-couplet` & `#kural-initial-seal` strictly Tamil; localize scholar labels (`மு.வ.` / `Mu. Va.`, etc.) reactively |
| 5 | **Medium** | Broken relative `<a href="../s1-start-unified.../...">` back links & toast-only mockups | `child_*` L674, `youth_*` L564, `adult_*` L445; inline 3-Kural `KURALS` JS arrays in all 12 `S2`–`S4` files | Clicking `←` 404s; Story/Game/Quiz/Search/AI buttons only show 2s toasts | Route all navigation & modals natively via `BackendController` over all `1,330` SQLite Kurals |
| 6 | **Medium** | Audio/speed asymmetries, non-button `<span>`, and Dark offline-dot colors | `S3` `SPEEDS = ['1.0x', '1.25x', '0.85x']` vs `S4` `['0.75x', '1.0x', '1.25x']`; `S4` `#pi-speed-chip` L504 is a `<span>`; `S3`/`S4` lack mute buttons; Dark offline dot is gold (`#E6C068`) on `S3` & crimson (`#8F1D24`) on `S4` | Inconsistent speeds, inaccessible speed chip, no mute on `S3`/`S4`, crimson dot looks like an error | Standardize `[0.75x, 1.0x, 1.25x]` `<button>` controls + mute toggle across `S2`–`S4`; use `#2ECC8F` (`dark-success`) for all offline ready dots |
| 7 | **Low** | Duplicate escaped CSS in `<style data-stitch-css-vh="...">` | All **16/16** files (`~10–15 KB` duplicate escaped CSS inside HTML attribute) | Unnecessary asset bloat & malformed `<style>` attribute | Strip completely when compiling `crates/kural-app/assets/app.css` |

---

## 7. Database Readiness & Phase 2 Gap Audit

### 7.1 Verified Current State (`cargo run --bin kural_cli -- stats` + FTS5 Audit)

```text
Database Stats (data/thirukkural.db):
  build_mode:      full (1330/1330 enriched)
  integrity_check: ok
  file_size:       30.79 MB (32288768 bytes)
  divisions:       3
  iyals:           9
  adhikarams:      133
  kurals:          1330   (meaning_parimel: 1328/1330; NULL on #395, #648)
  kural_child:     1330
  kural_youth:     1330
  kural_adult:     1330
  tags:            20
  kural_tags:      4856
  knowledge_base:  45
  kurals_fts:      1330
  knowledge_fts:   45
```

### 7.2 Phase 2 Delta Summary

- **Already Complete (100% Offline Pure Rust)**:
  - `crates/kural-enricher` has zero cloud/HTTP dependencies and deterministically synthesizes all `1,330/1,330` `kural_child`, `kural_youth`, `kural_adult`, and `4,856` `kural_tags` rows offline in `~1.4s`.
- **Remaining Work for Phase 2**:
  1. **Semantic Vector Index (`model2vec-rs`)**: Add `kural_embeddings (kural_number INTEGER PRIMARY KEY, vec_i8 BLOB NOT NULL, norm REAL NOT NULL)` and `knowledge_embeddings (kb_id INTEGER PRIMARY KEY, vec_i8 BLOB NOT NULL, norm REAL NOT NULL)` to `schema.rs`, implement `src/bin/index_embeddings.rs` (`potion-multilingual-128M-int8` with deterministic offline fallback when model files are absent), and add `< 2 ms` INT8 cosine similarity + **RRF** fusion in `kural-db`.
  2. **Persistent User State Store (`data/user_state.db`)**: Implement `UserStateStore` (`preferences`, `kids_progress`, `youth_progress`, `adult_bookmarks`) separate from read-only `thirukkural.db`.
  3. **Runtime Dependency Hygiene in `crates/kural-db`**: Gate build-time `reqwest` + `tokio` (used only by `src/bin/fetch_canonical.rs`) behind an optional Cargo feature (`fetcher`, `required-features = ["fetcher"]`) so the runtime `kural-db` library has **zero** HTTP client dependencies.
