# UrDevice: product

UrDevice joins a user's own devices (Windows, macOS, Linux, Android) into one network over the local network. There is no cloud server. All devices are equal peers; each feature has a Host (the device that provides or is controlled) and a Controller or Target.

Hackathon: Kiroween 2026, category Frankenstein. Deadline: 4 December 2026 14:00 Pacific (5 December 06:00 WITA). Feature work stops 3 December 06:00 WITA. The last two days are for the video, README, demo, and submission.

The spec is the source of truth: [requirements](../specs/urdevice/requirements.md), [design](../specs/urdevice/design.md), [tasks](../specs/urdevice/tasks.md). When a decision changes, update the requirement first, then the design, then the tasks.

## Decisions already made

- Permissions: a newly paired device has every capability "Not set". The first request shows a prompt: Allow always, Ask every time, or Deny (the feature does not start). Terminal and file changes show a warning, and "Allow always" needs one extra confirmation (R5.2).
- Speaker (R31): audio from a source device plays on a target device over the existing session.
- Demo target: 2 real devices. The app is still built to support many paired devices.
- Not in v1: iOS, use over the internet, relay or NAT traversal.

## Owner rules

- The owner decides goals and scope. Do not add features, delete data, or merge code without the owner's explicit approval.
- Do the review of a pull request only when the owner asks to merge it.
- Review and merge are two steps. After the review, report every finding (blocking and minor, including inconsistencies outside the diff) to the owner and wait for an explicit go. Merge only after that go. Never review and merge in one step.
- Do not claim in docs or the write-up that a Kiro feature was used unless it was.
