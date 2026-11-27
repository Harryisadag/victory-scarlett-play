// Report relay for victory-scarlett-play.
//
// The launcher POSTs an opt-in report (hardware, performance, the end of the log, a few small screenshots) to this
// Worker; the Worker checks it and forwards it to a private Discord channel. The Discord webhook address is a Worker
// secret (DISCORD_WEBHOOK) and never appears in the launcher or the repository, so the public .exe holds only this
// Worker's address, which can do nothing but post size-checked, rate-limited reports.

const MAX_BODY = 4 * 1024 * 1024; // whole request
const MAX_LOG = 60 * 1024; // end of the game log
const MAX_SHOTS = 4;
const MAX_SHOT = 700 * 1024; // one JPEG
const TEXT = 1000; // any one text field

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (request.method === 'GET' && url.pathname === '/') {
      return new Response('victory-scarlett-play report relay\n', { status: 200 });
    }
    if (request.method !== 'POST' || url.pathname !== '/report') {
      return new Response('not found\n', { status: 404 });
    }
    if (!env.DISCORD_WEBHOOK) {
      return new Response('relay not configured\n', { status: 503 });
    }
    const length = Number(request.headers.get('content-length') || '0');
    if (!length || length > MAX_BODY) {
      return new Response('too large\n', { status: 413 });
    }

    // Rate limit per sender address (and per install id below): a few reports a minute at most.
    const ip = request.headers.get('cf-connecting-ip') || 'unknown';
    if (env.LIMITER) {
      const { success } = await env.LIMITER.limit({ key: ip });
      if (!success) return new Response('slow down\n', { status: 429 });
    }

    let report;
    try {
      const raw = await request.text();
      if (raw.length > MAX_BODY) return new Response('too large\n', { status: 413 });
      report = JSON.parse(raw);
    } catch {
      return new Response('bad json\n', { status: 400 });
    }
    if (typeof report !== 'object' || report === null || report.kind !== 'victory-scarlett-play-report') {
      return new Response('not a report\n', { status: 400 });
    }
    const id = clean(report.install_id, 40);
    if (env.LIMITER && id) {
      const { success } = await env.LIMITER.limit({ key: 'id:' + id });
      if (!success) return new Response('slow down\n', { status: 429 });
    }

    const hw = report.hardware || {};
    const perf = report.performance || {};
    const fields = [
      ['Launcher', clean(report.launcher_version, 40)],
      ['Build', clean(report.build, 60)],
      ['Preset', clean(report.preset, 60)],
      ['Outcome', clean(report.outcome, 120)],
      ['CPU', clean(hw.cpu, 120)],
      ['GPU', `${clean(hw.gpu, 120)} (${num(hw.vram_gb)} GB, driver ${clean(hw.driver, 40)})`],
      ['RAM', `${num(hw.ram_gb)} GB, commit limit ${num(hw.commit_limit_gb)} GB`],
      ['Windows', clean(hw.windows, 80)],
      ['Session', `${num(perf.minutes)} min`],
      ['FPS', `avg ${num(perf.fps_avg)} · 1% low ${num(perf.fps_low)} · heavy scenes (>3000 draws) ${num(perf.fps_heavy)}`],
      ['Memory', `peak ${num(perf.peak_private_gb)} GB private`],
    ].filter(([, v]) => v && v !== 'undefined');

    const embed = {
      title: `Report from ${id ? id.slice(0, 8) : 'unknown'}`,
      color: report.outcome && /crash|lost|error/i.test(report.outcome) ? 0xd9534f : 0x5cb85c,
      fields: fields.map(([name, value]) => ({ name, value: String(value).slice(0, TEXT), inline: name !== 'Outcome' })),
      footer: { text: clean(report.note, 200) || 'victory-scarlett-play' },
      timestamp: new Date().toISOString(),
    };

    const form = new FormData();
    const files = [];
    const log = typeof report.log_tail === 'string' ? report.log_tail.slice(-MAX_LOG) : '';
    if (log) files.push(['log.txt', new Blob([log], { type: 'text/plain' })]);
    const shots = Array.isArray(report.screenshots) ? report.screenshots.slice(0, MAX_SHOTS) : [];
    for (const [i, s] of shots.entries()) {
      if (typeof s !== 'string') continue;
      let bytes;
      try {
        bytes = Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
      } catch {
        continue;
      }
      // JPEG only (starts FF D8 FF), small.
      if (bytes.length > MAX_SHOT || bytes[0] !== 0xff || bytes[1] !== 0xd8 || bytes[2] !== 0xff) continue;
      files.push([`shot-${i + 1}.jpg`, new Blob([bytes], { type: 'image/jpeg' })]);
    }
    form.append(
      'payload_json',
      JSON.stringify({
        username: 'victory-scarlett-play',
        embeds: [embed],
        allowed_mentions: { parse: [] },
        attachments: files.map(([name], i) => ({ id: i, filename: name })),
      }),
    );
    files.forEach(([name, blob], i) => form.append(`files[${i}]`, blob, name));

    const sent = await fetch(env.DISCORD_WEBHOOK, { method: 'POST', body: form });
    if (!sent.ok) {
      return new Response(`discord refused (${sent.status})\n`, { status: 502 });
    }
    return new Response('thanks\n', { status: 200 });
  },
};

// Text from the report: no mentions, no markdown links, bounded.
function clean(v, max) {
  if (typeof v !== 'string') return '';
  return v.replace(/@/g, '@​').replace(/[\u0000-\u001f]/g, ' ').slice(0, max).trim();
}

function num(v) {
  return typeof v === 'number' && Number.isFinite(v) ? Math.round(v * 10) / 10 : '?';
}
