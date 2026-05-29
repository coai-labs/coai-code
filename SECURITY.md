# Security Policy

> 中文版: [SECURITY.zh-CN.md](./SECURITY.zh-CN.md)

## Supported Versions

We provide security patches for the following versions of CoAI Code:

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |

Older pre-release versions are not supported. Please upgrade to the latest release before reporting a security issue.

## Reporting a Vulnerability

**Please do not open a public GitHub issue for security vulnerabilities.**

Report security issues by sending an email to **chonebz@gmail.com** with:

- A description of the vulnerability and its potential impact
- Steps to reproduce or proof-of-concept (if applicable)
- Affected versions
- Any suggested remediation, if you have one

### What to expect

| Timeline     | Action                                                                 |
| ------------ | ---------------------------------------------------------------------- |
| 48 hours     | Acknowledgment of your report                                          |
| 7 days       | Preliminary assessment and severity rating                             |
| 30–90 days   | Patch development, testing, and coordinated disclosure (severity-based)|

We follow responsible disclosure. Once a fix is ready we will:

1. Prepare a patch release.
2. Notify the reporter privately so they can verify the fix.
3. Publish a GitHub Security Advisory and release simultaneously.
4. Credit the reporter (unless they prefer to remain anonymous).

We ask reporters to keep vulnerability details private until the coordinated disclosure date.

## Scope

This policy covers the `coai` binary and its published library code. Third-party dependencies are out of scope; please report those issues upstream to the respective maintainers.

## Out of Scope

The following are **not** considered security vulnerabilities for the purposes of this policy:

- Issues requiring physical access to the user's machine
- Social engineering of project contributors
- Vulnerabilities in end-user environments not attributable to `coai` / `coai-code`
