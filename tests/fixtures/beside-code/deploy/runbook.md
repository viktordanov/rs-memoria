# Deploy runbook

Use this runbook to release Ledger. The [deploy guide](README.md) lists the files.

<!-- memoria:section id="release" files="deploy.sh" -->
## Release

1. Run `./deploy.sh` from the `deploy/` folder.
2. Open `http://ledger.example.com:8080/health`. The release succeeded when the page shows `ok`.
<!-- /memoria:section -->

<!-- memoria:section id="settings" files="config.toml" -->
## Settings

`config.toml` names the server in `host` and the port in `port`.
<!-- /memoria:section -->

## Release window

Release on a weekday between 09:00 and 16:00 UTC.
