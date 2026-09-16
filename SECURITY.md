# Security policy

## Supported versions

Security fixes are applied to the latest release and the `main` branch. Older
releases may not receive patches.

## Reporting a vulnerability

Please do not open a public issue for vulnerabilities that could cause data
loss, escape the cleanup allowlist, follow symlinks, execute an unintended
command, expose private paths, or bypass a confirmation or process check.

Use the repository's **Security → Report a vulnerability** form to send a
private report. If private vulnerability reporting is not yet enabled, contact
the maintainer privately through the profile linked from the repository owner.

Include:

- the affected version or commit;
- the macOS version and architecture;
- whether the issue is in analysis, interactive cleanup, or unattended mode;
- a minimal reproduction using disposable test directories; and
- the possible impact.

Remove usernames, home paths, filenames, tokens, cookies, and other personal
data from screenshots and logs. Do not test a suspected path-escape or deletion
bug against data you cannot afford to lose.

You can expect an acknowledgement within seven days. The maintainer will
coordinate validation, remediation, and disclosure timing with the reporter.
