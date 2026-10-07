# con Design Language

## Purpose

This document defines the design principles that should govern every prototype and implementation decision for **con**.

It is the shortest version of the brief: what the product should feel like, what visual habits it should follow, and what it should avoid.

This guide is meant to be read first, then used together with:

- `docs/design/con-ux-product-spec.md`
- `docs/design/con-ui-visual-spec.md`

---

## Implementation Foundations

| Element | Choice | Notes |
|---------|--------|-------|
| **Console font** | IoskeleyMono | Embedded. User-facing config uses `Ioskeley Mono`; GPUI text normalizes to the registered `IoskeleyMono` family; terminal backends receive concrete font names only. |
| **UI font** | `.SystemUIFont` | GPUI virtual alias for the platform UI font. Valid for GPUI-rendered UI, not for terminal renderer config. |
| **Default theme** | Flexoki Light | Dark available as Flexoki Dark. |
| **Icons** | Phosphor Icons | phosphoricons.com. SVGs at `assets/phosphor/`. |
| **Surfaces** | Borderless | No `border_1()` etc. Opacity-based fills only. |
| **Elevation** | Shadowless | No `shadow_sm()` etc. Bg opacity for depth. |

---

## Source Material

This design language is based on four inputs:

1. **con's product vision and early implementation**
   - `DESIGN.md`
   - `crates/con-app/src/workspace/`
   - `crates/con-app/src/ghostty_view.rs`
   - `crates/con-app/src/input_bar.rs`
   - `crates/con-app/src/agent_panel.rs`
   - `crates/con-app/src/settings_panel.rs`
2. **Terminal-native interaction patterns**
   - terminal structure and discoverability
   - command composition and action affordances
   - keyboard-forward UX patterns
3. **Nowledge Labs reference UI (production app)**
   - restraint, spacing, hierarchy, and surface treatment
   - elegant light mode with pure white foundation
   - segmented controls for mode switching
   - hairline borders and nearly invisible dividers
   - lavender/purple accent system
   - star ratings and tag pill components
   - keyboard shortcut (kbd) badge styling
   - all-caps section labels at 11px
4. **Nowledge Graph design guide**
   - silence over noise
   - typography as hierarchy
   - monochrome-by-default surfaces
   - opacity-based neutral system
   - progressive disclosure

The goal is not to imitate any one reference. The goal is to produce a design language that is unmistakably right for **con**.

---

## Dual Theme Philosophy

con is light-first (Flexoki Light default) but must excel in both themes.

### Light Mode (Default) - Apple Style

- **Base**: Pure white `#ffffff`
- **Surfaces**: Black at 2-6% opacity (`bg-black/[0.02]` to `bg-black/[0.06]`)
- **Text primary**: Black at 100%
- **Text secondary**: Black at 60%
- **Text muted**: Black at 25-40%
- **Borders**: Black at 4-6% opacity (nearly invisible)
- **Interactive hover**: Black at 3-6% opacity lift
- **Accent**: Blue `#2563eb` for agent/AI, amber for warnings

### Dark Mode - Apple Style

- **Base**: `#0a0a0b` (near-black, not gray)
- **Surfaces**: White at 2-8% opacity (`bg-white/[0.02]` to `bg-white/[0.08]`)
- **Text primary**: White at 100%
- **Text secondary**: White at 60%
- **Text muted**: White at 30-40%
- **Borders**: White at 4-6% opacity (nearly invisible)
- **Interactive hover**: White at 4-6% opacity lift
- **Accent**: Blue `#3b82f6` for agent/AI, amber for warnings

### Key Rules

- On transparent tab chrome, use solid foreground/muted foreground theme tokens
  for readable titles and identity icons. Do not further fade those glyphs with
  alpha; retain translucency on the surface instead.
- **No shadows** - Use opacity-based fills for elevation
- **No borders by default** - Only for strong semantic separation
- **Opacity-based system** - All neutrals derived from white/black with opacity
- **Single accent color** - Blue for AI/agent, used sparingly

### Light Mode Details (Nowledge-inspired)

Following the Nowledge Graph Design System's **borderless, colorless** philosophy:

**Color system (opacity-based)**

- Background: Pure white (`#ffffff`)
- Cards/Panels: `bg-black/[0.02]` - barely visible lift
- Hover states: `bg-black/[0.03]` - subtle feedback
- Borders: `border-black/[0.04]` to `border-black/[0.08]` - hairline
- Text primary: `text-black` or `rgba(0,0,0,0.87)`
- Text secondary: `text-black/80`
- Text muted: `text-black/40` to `text-black/50`
- Text subtle: `text-black/30`

**Key patterns**

- Avoid gray-named colors (`bg-gray-50`); use transparency instead
- Prefer no borders (v0.6 direction) - rely on spacing and typography
- Primary actions: `bg-black/[0.75]` with white text
- Section labels: uppercase, tracking-wider, `text-black/30`
- Cards: minimal or no border, subtle fill hover
- Inputs: white background with `border-black/[0.08]`, focus `border-black/[0.15]`

### Theme Switching

- Should switch instantly, without visible UI churn
- All semantic tokens must work in both themes
- Contrast ratios must meet accessibility in both modes

---

## North Star

con should feel like:

- a serious native terminal first
- a built-in AI harness second
- a calm, premium workstation throughout

The product should never feel like a chat app wrapped around a shell.
It should feel like the best place to do terminal-native work in the AI era.

---

## Core Principles

### 1. Terminal First

The terminal is the main stage.
Every other surface exists to support, frame, or clarify terminal work.

### 2. Silence Over Chrome

If a border, icon, fill, label, or effect does not increase orientation, confidence, or usability, remove it.
The product should feel considered, not decorated.

### 3. Typography Is Hierarchy

Use text size, weight, opacity, and spacing to create structure before reaching for boxes and borders.
The terminal already contributes visual density; the UI around it must stay quiet.

### 4. Monochrome by Default, Color by Meaning

The default visual language is neutral and opacity-driven.
Accent color is reserved for meaning:

- focus
- active selection
- success / running
- warning / approval
- error / danger
- assistant presence where useful

### 5. Dense, but Never Cramped

con should support serious workflows and high information density, but it must never feel packed or breathless.
Spacing is a functional tool, not empty luxury.

### 6. Trust Through Inspectability

As the agent gets more powerful, the UI must become more legible, not more magical.
Users should always understand:

- what context is attached
- what the agent plans to do
- what tool calls will run
- which pane or session is affected
- what changed afterward

### 7. Progressive Disclosure

Advanced controls, debug information, and low-frequency settings should stay out of the primary path.
Reveal depth only when it is useful.

### 8. Native Motion

Motion should be smooth, restrained, and low-travel.
No flourish. No bounce. No visual drama.
Just clarity and continuity.

---

## Product Character

### Desired qualities

- calm
- precise
- native
- sharp
- quiet
- fast
- trustworthy
- premium

### Undesired qualities

- chatty
- glossy
- decorative
- dashboard-heavy
- IDE-cluttered
- toy-like
- mysterious

---

## What Makes con Different

Unlike a knowledge app or dashboard product, con has to solve for:

- live terminal focus
- PTY truth and shell correctness
- split panes and tabs
- SSH and tmux awareness
- command execution and approvals
- compatibility with external agent CLIs and other terminal-native workflows

That means con needs:

- stronger focus cues than a content app
- slightly firmer interaction boundaries than a borderless data view
- clearer state signaling around active panes, approvals, and remote context
- a deliberate blend of system sans for UI and mono for terminal/code content

The result should carry the **taste level** of the Nowledge reference system, while becoming darker, faster, and more operational.

---

## Design Rules of Thumb

When evaluating a new screen or component, ask:

1. Does the terminal still feel like the center of gravity?
2. Could this hierarchy be solved with typography and spacing instead of another container?
3. Is color being used for meaning, or just for decoration?
4. Is the interaction obvious without becoming loud?
5. Does this increase trust, especially for agent actions?
6. Could advanced detail be hidden by default?
7. Does the motion make the product feel faster and calmer?

If the answer to several of these is “no,” the design is probably drifting.

---

## Final Statement

The target feeling for con is:

**an open-source native terminal with the restraint and polish of a world-class productivity app, but the confidence, speed, and truthfulness required by serious terminal work.**

---

## Apple-Level Design Philosophy (v0.8 Final)

### Core Visual Principles

1. **Borderless, shadowless surfaces** - Use opacity-based fills instead of borders/shadows
2. **Pure color system**:
   - Dark: `#0a0a0a` pure black base with white opacity overlays (3-8%)
   - Light: Pure white `#ffffff` content, `#fafafa` chrome with black opacity overlays (2-6%)
3. **Direct manipulation** - Click on elements to select/toggle, not separate controls
4. **Typography as hierarchy** - Size, weight, and opacity create structure without decoration
5. **Circular send buttons** - Apple Messages style for primary action (32px diameter)
6. **Minimal chrome** - Window chrome and sidebar barely distinguishable from content

### Apple System Blue

Primary accent color throughout: `#007AFF` (Apple's system blue)

- Used for: Agent mode, selections, primary actions, links
- Not used for: Decorative purposes, backgrounds, borders

### Pane Selection (Multi-Pane Targeting) - Final Design

When multiple panes exist, users need to select which panes receive commands:

**Pane Header Checkbox**

- Size: 15x15px with 4px border-radius (not fully round)
- Selected: solid `#007AFF` with 9px white bold checkmark
- Unselected: 1px border at 15% opacity (dark) or 12% (light), transparent interior
- Hover: border intensifies to 25% (dark) or 20% (light)
- Click toggles selection without changing active pane

**Status Bar Target Pills**

- Located in right side of status bar
- Height: 22px, fully rounded (rounded-full)
- Font: 10px semibold
- Selected: solid `#007AFF`, white text, 9px checkmark icon
- Unselected: no fill, 35% text opacity, hover shows 4% fill
- "ALL" badge: amber-500/15 background (dark) or amber-100 (light), amber text

**Behavior**

- Default: active pane only (no explicit selection needed)
- Click pane header checkbox OR status bar pill to toggle
- Clear selection returns to active-pane-only mode
- Broadcast indicator shown when 2+ panes selected

### Input Area Design (Production-Ready, Final v0.8)

The input area is the most critical interactive surface. Every pixel matters.

**Layout Structure**

1. Status bar (36px height) - above input, contains CWD/git + pane targets
2. Input container - rounded-2xl with textarea + send button
3. Hints bar - minimal text below input

**Status Bar**

- Height: 36px (h-9)
- Border-top: 4% opacity
- Left: CWD + Git branch (11px font, 30% opacity, font-medium)
- Right: Pane target pills (only when multiple panes exist)
- Icons: 12px, same opacity as text

**Input Container**

- Padding: 20px horizontal (px-5), 8px top (pt-2), 20px bottom (pb-5)
- Inner container: rounded-2xl, 16px horizontal padding, 14px vertical
- Background: white/[0.03] dark, black/[0.02] light
- Focus-within: background lifts to white/[0.05] or black/[0.04]
- Flex layout: items-end, gap-3

**Textarea**

- Font: 14px system sans, leading-relaxed
- Placeholder: 25% opacity (not 50%)
- Min-height: 22px, max-height: 120px
- Auto-resize based on content
- No border, transparent background

**Send Button (Apple Messages Style)**

- Size: 32x32px (w-8 h-8)
- Shape: fully rounded (rounded-full)
- Active states:
  - Agent mode: `#007AFF` background, white arrow
  - Shell mode: white/black solid, inverted arrow
- Inactive: 4% fill (dark) or 3% (light), 15% text
- Interaction: active:scale-95 for tactile feedback
- Loading: CircleNotch spinner animation

**Hints Bar**

- Margin-top: 10px (mt-2.5)
- Padding: 4px horizontal (px-1)
- Font: 10px medium
- Left: "Enter to send" + "Shift+Enter for newline" (15% opacity)
- Right: Mode indicator - "Agent" (blue), "Shell", or "Auto"

**Pane Target Pills (Status Bar)**

- Height: 22px, fully rounded
- Font: 10px semibold
- Gap between pills: 6px
- Selected: `#007AFF` solid, white text, 9px checkmark
- Unselected: transparent, 35% text, hover 4% fill
- Broadcast "ALL" badge: amber colors, 9px font, bold tracking

**Transitions**

- All color/opacity changes: transition-all (150ms default)
- Button press: active:scale-95
- No shadows anywhere

## Component Patterns (from Nowledge Reference)

### Segmented Controls

Used for mode switching (Theme, Font size).

- Soft container background using opacity (`bg-white/[0.06]` dark, `bg-black/[0.04]` light)
- Active segment: slightly stronger fill with optional shadow in light mode
- Inactive: muted text, no background
- Compact sizing: 32px height, 12px font
- Rounded container (`rounded-xl`)

### Tag Pills / Chips

Used for context, categories, entities, filters.

- Soft border radius (10-12px)
- Light background in light mode, subtle fill in dark
- 11-12px text
- Optional leading indicator dot for status

### Section Labels

- ALL CAPS
- 10-11px size
- Muted foreground (50% opacity equivalent)
- 20-24px bottom margin

### Keyboard Shortcuts (kbd)

- Inline display with 2px border radius
- Subtle border and background
- Monospace or medium-weight sans
- Grouped horizontally with 4px gap

### Cards

- Pure white in light mode, elevated surface in dark
- Very subtle border (1px at 8-10% opacity)
- 12-16px padding
- Optional hover state: slightly stronger border

### Toggle Switches

- Dark filled track when on
- White/light thumb
- 44px width, 24px height
- 100ms transition

### Search Inputs

- Soft background (secondary surface)
- Search icon on left, subdued
- Clear right padding for action buttons
- 40-44px height

### Lists with Metadata

- Title: 14px medium weight
- Description: 12-13px, muted
- Metadata row: 11px, very muted
- Tags inline below description
- Right-aligned actions (ratings, timestamps)
