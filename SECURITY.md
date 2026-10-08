# Security Policy

## Reporting a vulnerability

Please **do not** report security vulnerabilities through public GitHub issues.

Report a potential vulnerability using the
[**Report a vulnerability** button](https://github.com/ReQurv/requrv-bridge/security/advisories/new)
on the GitHub Security tab (Private Vulnerability Reporting), or by email to
[security@requrv.io](mailto:security@requrv.io).

You should receive an acknowledgment within 48 hours, and a substantive response
within 7 days. We will keep you informed throughout the process. Please allow
us at least 90 days to develop and ship a fix before any public disclosure.

## Scope

This policy covers the open-source ReQurv Bridge application (this repository).
The AI Hive gateway itself is a separate, closed service: report gateway issues
to [security@requrv.io](mailto:security@requrv.io) as well.

## Known design considerations

Be aware of these intentional design choices before reporting them as issues:

- **The AI Hive API key is stored in plaintext** in `hive.json` inside the app's
  user configuration directory (a `.bak` backup is written before every
  overwrite). The key is user-local and is required by the agents this app
  launches (several of them only accept the key from a config file or an
  environment variable). We are evaluating OS keychain integration; please
  discuss it in an issue before implementing it.
- **The app rewrites agent configuration files** (OpenCode, Codex, ChatGPT.app)
  to point them at AI Hive. Originals are always kept as one-shot `.bak` /
  `.hive.bak` backups and can be restored from the app.
