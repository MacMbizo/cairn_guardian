# Security policy

## Supported versions

Cairn Guardian is pre-release. Security fixes go to the latest commit on `master`.

## Reporting a vulnerability

Please **do not** open a public issue for a security problem.

Use GitHub's [private vulnerability reporting](https://github.com/MacMbizo/cairn_guardian/security/advisories/new) instead. Include:

- what the problem is and which part of Cairn it affects;
- the steps to reproduce it, or a proof of concept;
- the impact you expect, for example data loss, data exposure or privilege escalation.

You should get an acknowledgement within a week. Please give us reasonable time to release a fix before you disclose the problem publicly.

## Scope

Reports we especially want:

- any path that changes, moves or deletes user files outside the batch and undo pipeline, or that bypasses the quarantine;
- weaknesses in catalog encryption, key sealing, recovery phrases or Shamir key shares;
- ways to reveal masked sensitive content without the Windows credential prompt;
- any network traffic from the engine or its model crates.
