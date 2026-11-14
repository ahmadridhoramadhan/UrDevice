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

Spec changes made during the hackathon: a first-use permission prompt (R5.2), and a "use another device as a speaker" feature (R31).

## Kiro Crew

A Kiro Crew bot ("PM UrDevice") plans the work, splits it into small tasks, tracks progress against the 5 December 2026 deadline, reviews pull requests, and reports schedule risk. The project owner keeps all decisions: no new features, data deletion, or merges happen without approval.

## Steering docs

**TODO** (none yet). Planned: project conventions for Rust, Kotlin, and security rules.

## Agent hooks

**TODO** (none yet).

## MCP

**TODO.** The GitHub MCP server is used by Kiro Crew worker sessions to read and merge pull requests.

## Property-based testing

**TODO.** Planned targets from the spec: path cleaning for file transfer, and file-browser paths that must never escape a shared folder (tasks T3.6 and T14.2).

## Kiro IDE, CLI, and Crew

Two Kiro products were used:

- **Kiro Web:** the owner used it to write the spec (requirements, design, tasks). At that time the owner did not yet know how Kiro works in detail and asked for the spec to be placed in `docs/`. It was moved to `.kiro/specs/urdevice/` later.
- **Kiro Crew:** the owner continued here for end-to-end work. The Crew bot plans the work, splits tasks, reviews and merges pull requests (through the GitHub MCP server), and tracks the deadline.

The Kiro IDE and CLI were not used.

**TODO:** how the two differed in practice (speed, control, quality of results), in the owner's words.

## Third-party licenses

UrDevice's own code is MIT. The desktop UI uses [Slint](https://slint.dev) under its royalty-free license, with attribution.
