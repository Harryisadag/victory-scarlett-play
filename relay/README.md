# Report relay

The launcher's reports go here first, then to a private Discord channel. The Discord webhook address is stored only
as a secret inside the Worker; the public launcher knows only the Worker's address.

## Set it up (once)

1. **Discord webhook.** In your Discord server: create a private text channel (e.g. `#reports`) → Edit Channel →
   Integrations → Webhooks → New Webhook → Copy Webhook URL. Keep that URL to yourself.
2. **Cloudflare account.** Sign up free at https://dash.cloudflare.com/sign-up.
3. **Install Node.js** (LTS) from https://nodejs.org if you don't have it.
4. In this folder (`relay`), in a terminal:

   ```
   npx wrangler login
   npx wrangler deploy
   npx wrangler secret put DISCORD_WEBHOOK
   ```

   `login` opens the browser; `deploy` prints the Worker's address (`https://victory-scarlett-relay.<you>.workers.dev`);
   `secret put` asks for the webhook URL - paste it there, not in any file.
5. Check it: opening the Worker's address in a browser shows `victory-scarlett-play report relay`.
6. Put the Worker's address in `apps/launcher/src/report.rs` (`RELAY`), or set `VSP_RELAY` when building.

Limits: 4 MB a report, at most 4 JPEG screenshots of 700 KB, 60 KB of log, 6 reports a minute per address and per
install. Reports are sent only if the player said yes on the launcher's first start.
