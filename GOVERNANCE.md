# ORP Governance

ORP is an open-source project governed by its community of contributors. This document describes how decisions are made, how the project is structured, and how you can take on more responsibility over time.

---

## Table of Contents

1. [Values](#values)
2. [Contributor Ladder](#contributor-ladder)
3. [Technical Steering Committee (TSC)](#technical-steering-committee-tsc)
4. [Decision-Making](#decision-making)
5. [RFC Process](#rfc-process)
6. [Project Maintenance](#project-maintenance)
7. [Code of Conduct](#code-of-conduct)
8. [Amendments](#amendments)

---

## Values

ORP is guided by four core values:

1. **Open.** The tools for understanding physical reality should be free. No dual licensing, no open-core bait-and-switch.
2. **Correctness over speed.** We prefer a slow, correct implementation over a fast, broken one. Benchmarks matter; so do security audits.
3. **Single-binary simplicity.** Every design decision must be evaluated against: does this make it harder to download and run ORP in under 5 minutes?
4. **Security by default.** Secure defaults, not secure options. ABAC, signing, and audit logging are on unless explicitly disabled.

---

## Contributor Ladder

ORP uses a four-level contributor ladder. Moving up is recognition, not a gatekeeping exercise.

### Level 1 — User

Anyone who downloads and uses ORP. No formal requirements.

**What you can do:**
- Open issues and feature requests
- Ask questions in Discord and GitHub Discussions
- Share use cases and feedback

---

### Level 2 — Contributor

Anyone who has had **at least one PR merged** into the main ORP repository.

**Requirements:** One merged PR (any type: code, docs, tests, CI).

**What you can do:**
- Everything a User can do
- Be assigned issues
- Have your PRs reviewed with higher priority
- Participate in RFC discussions
- Be listed in [CONTRIBUTORS.md](CONTRIBUTORS.md)

**How to become one:** Open a PR. When it merges, you're a Contributor.

---

### Level 3 — Committer

Contributors with a sustained record of high-quality contributions and demonstrated understanding of ORP's architecture and values.

**Requirements:**
- At least 10 merged PRs across multiple areas of the codebase
- Demonstrated ability to review PRs constructively
- Nominated by an existing Committer or TSC member
- Approved by a simple majority of the TSC (no active objections within 7 days)

**What you can do:**
- Everything a Contributor can do
- Merge PRs after they meet review requirements
- Triage and label issues
- Approve RFCs for standard changes
- Participate in TSC discussions (non-voting)
- Be listed in [COMMITTERS.md](COMMITTERS.md)

**Responsibilities:**
- Review PRs within 48 hours when assigned
- Uphold the project's code review standards
- Maintain awareness of the roadmap and ongoing RFCs

---

### Level 4 — TSC Member

The Technical Steering Committee consists of Committers who have taken on long-term project stewardship responsibilities.

**Requirements:**
- Active Committer for at least 6 months
- Demonstrated strategic contributions (roadmap input, RFC authorship, cross-cutting work)
- Nominated by an existing TSC member
- Approved by a supermajority (2/3) of the TSC

**What you can do:**
- Everything a Committer can do
- Vote on RFCs, architectural decisions, and project direction
- Vote on adding/removing Committers and TSC members
- Represent ORP in external settings (conferences, standards bodies)

**Responsibilities:**
- Attend TSC meetings (monthly, async by default)
- Actively participate in roadmap planning
- Review security-sensitive PRs
- Act as a tiebreaker for contested decisions

---

### Stepping Down

Life happens. If you can no longer fulfill your role, please post in `#governance` on Discord or open a GitHub Discussion. There is no shame in stepping back — emeritus status is honored and celebrated.

After 6 months of inactivity, TSC members will reach out. After 12 months of confirmed inactivity, a member may be moved to emeritus status (with their consent).

---

## Technical Steering Committee (TSC)

### Composition

The TSC starts with the founding contributors and grows as the community matures. Membership is listed in [TSC.md](TSC.md).

### Meetings

- **Frequency:** Monthly, async-first. A GitHub Discussion is opened for each meeting's agenda.
- **Synchronous option:** Optional video call for items that benefit from real-time discussion.
- **Notes:** Published in `docs/tsc-notes/` within one week of each meeting.

### Responsibilities

1. Set and maintain the project roadmap
2. Review and approve large RFCs
3. Manage the release process and versioning
4. Maintain project infrastructure (CI, release pipelines, package registries)
5. Resolve escalated disputes
6. Represent the project externally

---

## Decision-Making

ORP uses a tiered decision-making model based on the scope and reversibility of the change.

### Tier 1 — Lazy Consensus (most changes)

Used for: bug fixes, new connectors, documentation, test additions, minor refactors.

**Process:** Author opens a PR. Any Committer can approve and merge. No explicit vote required.

**Overriding:** Any Committer can block a merge by marking "Request Changes" with a specific objection. If the objection isn't resolved within 5 business days, escalate to Tier 2.

---

### Tier 2 — Committer Vote (architectural changes)

Used for: new crates, significant API changes, dependency additions/removals, performance architecture changes.

**Process:** Author opens an RFC (see below). Committers discuss in the RFC thread. After a 7-day comment period with no blocking objections, a Committer can approve it. If there are objections, a simple majority vote of Committers decides.

---

### Tier 3 — TSC Vote (major decisions)

Used for: breaking changes, security model changes, license changes, governance amendments, new TSC or Committer appointments.

**Process:** Author opens an RFC. A 14-day comment period. TSC votes. Supermajority (2/3) required for breaking changes and governance amendments; simple majority for other Tier 3 items.

**Emergency security patches:** A single TSC member can authorize a security patch to be merged immediately. The RFC/discussion happens retroactively within 48 hours.

---

## RFC Process

RFCs (Request for Comments) are the mechanism for proposing significant changes before implementation begins. They prevent wasted work and build consensus.

### When Is an RFC Required?

- New crate or major restructuring of an existing crate
- New ORP-QL syntax or query semantics
- Changes to the config file schema that affect existing deployments
- New security mechanisms or changes to existing ones
- Changes to the connector trait API
- Anything that would be a `BREAKING CHANGE` in a commit

When in doubt: write the RFC. It's a short document, not a contract.

### RFC Template

Create a file at `docs/rfcs/NNNN-short-title.md`:

```markdown
# RFC NNNN: Short Title

**Author:** Your Name
**Date:** YYYY-MM-DD
**Status:** Draft | Active | Accepted | Rejected | Superseded
**Tier:** 2 | 3

## Summary
One paragraph. What are you proposing and why?

## Motivation
What problem does this solve? Who benefits?

## Detailed Design
The meat. Be specific enough that someone could implement this from the RFC alone.

## Alternatives Considered
What else was considered and why was it rejected?

## Drawbacks
What are the costs of this change?

## Unresolved Questions
What still needs to be figured out?

## Implementation Plan
How will this be implemented? Any phasing?
```

### RFC Lifecycle

1. **Draft:** Author creates the RFC file and opens a PR. Title: `docs: RFC NNNN - short title`.
2. **Active:** PR is merged to `main` after a quick sanity review (format only, not content). Discussion happens in the GitHub Discussion linked from the RFC.
3. **Accepted / Rejected:** After the comment period and vote, the RFC status is updated and the PR implementing it can begin.
4. **Superseded:** A later RFC replaces this one.

---

## Project Maintenance

### Versioning

ORP follows [Semantic Versioning](https://semver.org/):

- **PATCH** (0.1.x): Bug fixes, performance improvements, no API changes
- **MINOR** (0.x.0): New features, backward-compatible API additions
- **MAJOR** (x.0.0): Breaking changes (rare; require Tier 3 decision)

### Release Cadence

- **Patch releases:** As needed (critical bugs, security fixes)
- **Minor releases:** Every 4–8 weeks, when significant features accumulate
- **Major releases:** When the TSC decides the accumulated breaking changes warrant one

### Long-Term Support (LTS)

Starting from v1.0.0, one minor release per year will be designated LTS and receive security patches for 18 months.

---

## Code of Conduct

ORP follows the [Contributor Covenant v2.1](https://www.contributor-covenant.org/version/2/1/code_of_conduct/).

**In short:** Be respectful, assume good faith, focus on technical merit. Harassment, discrimination, and bad-faith behavior will result in removal.

**Enforcement:** Report incidents to `conduct@orp.dev`. TSC members not involved in the incident will review within 48 hours.

---

## Amendments

This governance document can be amended by a Tier 3 TSC vote with a 14-day comment period. All amendments are recorded in the git history.

---

_ORP Governance v1.0 · 2026-03-26_
