# Demo Smoke Test

This folder contains an end-to-end smoke test for the browser demo using Playwright.

## What it checks

- Map add/update/delete mirrors from peer A to peer B
- Blob metadata write mirrors to peer B
- Collaborative text mirrors to peer B
- List item insert mirrors to peer B

## Run

From repository root:

```powershell
Set-Location web/smoke
npm install
npm run smoke
```

Or from `sdk-js`:

```powershell
Set-Location sdk-js
npm run smoke:demo
```

## Notes

- The script auto-starts `cargo run --bin activesync-server` if `127.0.0.1:7878` is not already listening.
- The script auto-starts `python web/serve.py 8080` if `127.0.0.1:8080` is not already listening.
- If either service is already running, it reuses it.
- On failure, the script prints peer status, state text, and recent event log lines for both peers.
