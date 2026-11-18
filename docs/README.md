# UrDevice documentation

## Spec

- [Requirements](../.kiro/specs/urdevice/requirements.md): what UrDevice must do, as user stories with acceptance criteria.
- [Design](../.kiro/specs/urdevice/design.md): architecture, protocol, security model, and how each platform implements each feature.
- [Tasks](../.kiro/specs/urdevice/tasks.md): the implementation plan, in phases.

## Kiro

- [How UrDevice used Kiro](kiro-usage.md): the write-up for the Kiroween submission.

## Design

The mockups are a guide, not a final design. Open the HTML files in a browser from a clone of the repo.

- [App layout](mockup/urdevice-layout.html): interactive layout for desktop and Android (two navigation options).
- [Onboarding](mockup/onboarding.md): the first-launch flow for desktop and Android, with a clickable mockup ([`onboarding.html`](mockup/onboarding.html)).

## Development

- [Development](development.md): prerequisites and build commands for each platform.
- [Threat model](security/threat-model.md): assets, trust boundaries, and mitigations for pairing, transport, and permissions.
- [Spike T0.3](spikes/t0.3-quic-android.md): QUIC and discovery between Android and a desktop on real Wi-Fi, with round-trip times.

Implementation should follow the tasks in order. When a decision changes, update the design first.
