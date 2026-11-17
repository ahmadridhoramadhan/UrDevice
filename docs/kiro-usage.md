# How UrDevice used Kiro

This is the write-up for the Kiroween 2026 submission (category: Frankenstein). It is a draft. Items marked **TODO** must be filled with what actually happened before submitting. Do not claim a feature that was not used.

## Why Frankenstein

UrDevice stitches together parts that do not normally fit: one Rust core shared by a Slint desktop app (Windows, macOS, Linux) and a native Kotlin Android app through UniFFI, with QUIC, mDNS, and per-platform input, audio, and screen adapters. All devices are equal peers on the local network, with no cloud server.

## Spec-driven development

The spec lives in [`.kiro/specs/urdevice/`](../.kiro/specs/urdevice/):

- [`requirements.md`](../.kiro/specs/urdevice/requirements.md): user stories with numbered acceptance criteria (R1 to R31).
- [`design.md`](../.kiro/specs/urdevice/design.md): architecture, protocol, security model, and platform adapters.
- [`tasks.md`](../.kiro/specs/urdevice/tasks.md): phased tasks, each linked to the requirements it covers.

The spec was first written in Kiro Web (see "Kiro IDE, CLI, and Crew" below).

How it helped: **TODO** (for example, how the task list kept work in small steps, and how changing a requirement updated the design first).

Spec changes made during the hackathon:

- A "use another device as a speaker" feature (R31).
- A first-use permission prompt (R5.2), later replaced after the UI review.
- A review of the owner's UI mockup against the spec (17 November 2026). It changed permissions to one Allow / Always Ask / Deny setting per capability with onboarding defaults (R5), added QR scanning on desktop (R3.10), Disconnect, latency, and battery (R4.7–R4.9), the Bluetooth LE banner (R6.4), media sources and seek (R11), "Expose on LAN" for 1 hour or always (R13.6), and desktop → Android notifications (R18). The decisions are listed in design §17. The interactive mockups are in [`docs/mockup/`](mockup/).

## Kiro Crew

A Kiro Crew bot ("PM UrDevice") plans the work, splits it into small tasks, tracks progress against the 5 December 2026 deadline, reviews pull requests, and reports schedule risk. The project owner keeps all decisions: no new features, data deletion, or merges happen without approval.

## Steering docs

Three steering files are in [`.kiro/steering/`](../.kiro/steering/): `product.md` (goal, decisions, owner rules), `tech.md` (stack, crates, checks before commit), and `security.md` (security rules for every change). They were written by the Kiro Crew bot from the spec and the repository, and the owner approved them.

On 17 November 2026, Kiro Web updated the three files to match the decisions from the UI review and added a fourth file, `ui.md`, which links the UI mockups and lists the UI behavior to keep.

The files use Kiro's inclusion modes:

| File | Inclusion | Why |
|---|---|---|
| `product.md` | `always` | Goal, deadline, decisions, and owner rules apply to every request |
| `security.md` | `always` | Security rules apply to every change |
| `tech.md` | `fileMatch` on Rust, Cargo, Slint, Android, and CI files | Build and style rules are only needed when code is touched |
| `ui.md` | `fileMatch` on Slint, desktop app, Android app, and mockup files | UI rules are only needed for UI work |

How they helped: **TODO** (after real use).

## Agent hooks

**TODO** (none yet).

## MCP

**TODO.** The GitHub MCP server is used by Kiro Crew worker sessions to read and merge pull requests.

## Property-based testing

**TODO.** Planned targets from the spec: path cleaning for file transfer, and file-browser paths that must never escape a shared folder (tasks T3.6 and T14.2).

## Kiro IDE, CLI, and Crew

Two Kiro products were used:

- **Kiro Web:** the owner used it to write the spec (requirements, design, tasks). At that time the owner did not yet know how Kiro works in detail and asked for the spec to be placed in `docs/`. It was moved to `.kiro/specs/urdevice/` later. Later the owner also used Kiro Web to review a UI mockup against the spec, update the spec and the mockups, update the steering files, and open and merge the pull requests (PR #9 and the follow-ups).
- **Kiro Crew:** the owner continued here for end-to-end work. The Crew bot plans the work, splits tasks, reviews and merges pull requests (through the GitHub MCP server), and tracks the deadline.

The Kiro IDE and CLI were not used.

How they differ, in the owner's words:

- **Kiro Web:** more limited but flexible. It can be used from a phone or anywhere, with no setup.
- **Kiro Crew:** can run locally or on a VM, and has many more features, but needs setup.

## Third-party licenses

UrDevice's own code is MIT. The desktop UI uses [Slint](https://slint.dev) under its royalty-free license, with attribution.
