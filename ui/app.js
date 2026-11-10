"use strict";

const T = window.__TAURI__;
const invoke = T.core.invoke;
const appWindow = T.window.getCurrentWindow();
const $ = (id) => document.getElementById(id);
const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];

const MAPS = {
  bigmap: "Customs", factory4_day: "Factory", factory4_night: "Night Factory", interchange: "Interchange",
  laboratory: "The Lab", lighthouse: "Lighthouse", rezervbase: "Reserve", shoreline: "Shoreline",
  woods: "Woods", tarkovstreets: "Streets", sandbox: "Ground Zero", sandbox_high: "Ground Zero 21+",
  labyrinth: "Labyrinth",
};
const ACTIVITY = ["In menu", "In raid", "In stash", "In hideout", "On the flea"];
const MODE_TEXT = { full: "Full stack", host: "Host only", join: "Game only" };
const STREAM_NAMES = ["server", "headless", "game"];

const state = {
  boot: null,
  settings: null,
  status: null,
  profiles: [],
  profileId: null,
  mode: "full",
  page: "dashboard",
  hist: { server: [], headless: [], game: [] },
  logs: [[], [], []],
  logSeq: [-1, -1, -1],
  logErr: [0, 0, 0],
  logSeen: [0, 0, 0],
  stream: 0,
  level: "all",
  cfg: { root: null, entries: [], open: null, stamp: null, original: "" },
};

// ============================ helpers ============================

const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
const mapName = (id) => MAPS[(id || "").toLowerCase()] || id || "Unknown";
const initials = (n) => (n || "?").replace(/[^A-Za-z0-9]/g, "").slice(0, 2).toUpperCase() || "?";

function human(n) {
  if (n >= 1e9) return (n / 1e9).toFixed(1).replace(/\.0$/, "") + "B";
  if (n >= 1e6) return (n / 1e6).toFixed(1).replace(/\.0$/, "") + "M";
  if (n >= 1e3) return (n / 1e3).toFixed(1).replace(/\.0$/, "") + "k";
  return String(n);
}
function bytes(b) {
  if (!b) return "-";
  const u = ["B", "KB", "MB", "GB"];
  let i = 0;
  while (b >= 1024 && i < u.length - 1) { b /= 1024; i++; }
  return (b >= 100 || i === 0 ? b.toFixed(0) : b.toFixed(1)) + " " + u[i];
}
function dur(s) {
  if (!s && s !== 0) return "-";
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60);
  if (h >= 24) return Math.floor(h / 24) + "d " + (h % 24) + "h";
  if (h) return h + "h " + m + "m";
  if (m) return m + "m " + (s % 60) + "s";
  return s + "s";
}
function ago(unix) {
  if (!unix) return "never";
  const s = Math.max(0, Date.now() / 1000 - unix);
  if (s < 90) return "just now";
  return dur(Math.floor(s)).split(" ")[0] + " ago";
}

function toast(msg, bad) {
  const t = $("toast");
  t.textContent = msg;
  t.classList.toggle("bad", !!bad);
  t.classList.add("show");
  clearTimeout(toast.t);
  toast.t = setTimeout(() => t.classList.remove("show"), 2400);
}

function confirmBox(title, body, yes = "OK", no = "Cancel") {
  return new Promise((resolve) => {
    $("modalTitle").textContent = title;
    $("modalBody").textContent = body;
    $("modalYes").textContent = yes;
    $("modalNo").textContent = no;
    $("modal").hidden = false;
    const done = (v) => { $("modal").hidden = true; $("modalYes").onclick = $("modalNo").onclick = null; resolve(v); };
    $("modalYes").onclick = () => done(true);
    $("modalNo").onclick = () => done(false);
  });
}

async function call(cmd, args) {
  try { return await invoke(cmd, args); }
  catch (e) { toast(String(e), true); throw e; }
}

// ============================ window + nav ============================

$("winMin").onclick = () => appWindow?.minimize();
$("winMax").onclick = () => appWindow?.toggleMaximize();
$("winClose").onclick = async () => {
  const s = state.status;
  const owns = s && (s.server.owned || s.headless.owned || s.game.owned);
  if (owns && !(await confirmBox("Quit Fika Control?", "This stops the server, headless and game that were started from here.", "Stop and quit"))) return;
  invoke("quit");
};

function goto(page) {
  state.page = page;
  $$(".rail-btn").forEach((b) => b.classList.toggle("active", b.dataset.page === page));
  $$(".page").forEach((p) => p.classList.toggle("active", p.id === "page-" + page));
  if (page === "console") { state.logSeen = [...state.logErr]; renderLog(true); updateBadges(); }
  if (page === "configs" && !state.cfg.root) loadConfigRoots();
}
$$(".rail-btn").forEach((b) => (b.onclick = () => goto(b.dataset.page)));
$$("[data-open]").forEach((b) => (b.onclick = () => call("open_folder", { which: b.dataset.open }).catch(() => {})));
$$("[data-goto-log]").forEach((b) => (b.onclick = () => { setStream(+b.dataset.gotoLog); goto("console"); }));

document.addEventListener("keydown", (e) => {
  if (e.ctrlKey && e.key.toLowerCase() === "s" && state.page === "configs") { e.preventDefault(); saveConfig(); }
  if (e.ctrlKey && e.key.toLowerCase() === "f" && state.page === "console") { e.preventDefault(); $("logSearch").focus(); }
  if (e.key === "Escape") $("whoMenu").hidden = true;
});

// ============================ boot + setup ============================

async function boot() {
  const b = await invoke("boot");
  applyBoot(b);
  state.mode = b.settings.launchMode || "full";
  state.profileId = b.settings.lastProfile;
  renderMode();
  if (b.hostProblem) showSetup();
  await refreshProfiles();
  pollStatus();
  setInterval(pollStatus, 1000);
  setInterval(pollLogs, 500);
  setInterval(pollLobby, 3000);
  setInterval(refreshProfiles, 15000);
}

function applyBoot(b) {
  state.boot = b;
  state.settings = b.settings;
  const v = b.versions;
  $("chipSpt").textContent = "SPT " + (v.sptHost || "?");
  $("chipFika").textContent = "Fika " + (v.fikaHost || "?");

  // only matters when both sides could be read
  const sptBad = v.sptHost && v.sptPlayer && v.sptHost !== v.sptPlayer;
  const fikaBad = v.fikaHost && v.fikaPlayer && v.fikaHost !== v.fikaPlayer;
  $("chipSpt").classList.toggle("bad", !!sptBad);
  $("chipFika").classList.toggle("bad", !!fikaBad);
  const parts = [];
  if (sptBad) parts.push(`SPT ${v.sptHost} on the host vs ${v.sptPlayer} in your game`);
  if (fikaBad) parts.push(`Fika ${v.fikaHost} on the host vs ${v.fikaPlayer} in your game`);
  $("mismatchBanner").hidden = !parts.length;
  $("mismatchText").textContent = parts.join(" · ") + ". Update the older one or you likely won't connect.";
  state.mismatch = parts;

  $("setHost").textContent = b.hostDir || "Not set";
  $("setHostNote").textContent = b.hostProblem || (b.hostFromExe ? "Detected from where the launcher sits." : "");
  $("setHostNote").className = b.hostProblem ? "bad" : "";
  $("browseHost").disabled = b.hostFromExe;
  $("setPlayer").textContent = b.playerDir || "Not set (host only)";
  $("setPlayerNote").textContent = b.playerProblem || (v.sptPlayer ? `SPT ${v.sptPlayer} · Fika ${v.fikaPlayer || "?"}` : "");
  $("setPlayerNote").className = b.playerProblem ? "bad" : "";

  $$("[data-set]").forEach((i) => (i.checked = !!b.settings[i.dataset.set]));
  $$("#displaySeg button").forEach((x) => x.classList.toggle("on", x.dataset.display === (b.settings.displayMode || "")));
  $("aboutText").textContent = `Fika Control ${b.appVersion} · by 20fpsguy`;
}

function showSetup() {
  const b = state.boot;
  $("setup").hidden = false;
  $("stepHost").classList.toggle("done", !b.hostProblem);
  $("setupHostText").textContent = b.hostDir || "Where the server and headless live.";
  $("setupHostBtn").hidden = b.hostFromExe;
  $("stepPlayer").classList.toggle("done", !!b.playerDir && !b.playerProblem);
  $("setupPlayerText").textContent = b.playerDir || "The copy you play on. Skip it if you only host.";
  $("setupDone").disabled = !!b.hostProblem;
}

async function browse(kind) {
  const path = await invoke("pick_folder");
  if (!path) return null;
  try {
    const b = await invoke("set_dir", { kind, path });
    applyBoot(b);
    $("setupErr").hidden = true;
    await refreshProfiles();
    if (state.cfg.root) loadConfigRoots();
    return b;
  } catch (e) {
    $("setupErr").textContent = String(e);
    $("setupErr").hidden = $("setup").hidden;
    toast(String(e), true);
    return null;
  }
}
$("setupHostBtn").onclick = async () => { await browse("host"); showSetup(); };
$("setupPlayerBtn").onclick = async () => { await browse("player"); showSetup(); };
$("setupDone").onclick = () => ($("setup").hidden = true);
$("browseHost").onclick = () => browse("host");
$("browsePlayer").onclick = () => browse("player");

// ============================ settings ============================

function saveSettings() { invoke("save_settings", { settings: state.settings }); }
$$("[data-set]").forEach((i) => (i.onchange = () => { state.settings[i.dataset.set] = i.checked; saveSettings(); }));
$$("#displaySeg button").forEach((b) => (b.onclick = () => {
  state.settings.displayMode = b.dataset.display;
  $$("#displaySeg button").forEach((x) => x.classList.toggle("on", x === b));
  saveSettings();
  toast(b.dataset.display ? `Game will start ${b.textContent.toLowerCase()}` : "Window mode left alone");
}));

// ============================ mode + profile ============================

function renderMode() {
  $$("#modeSeg button").forEach((b) => b.classList.toggle("on", b.dataset.mode === state.mode));
  $("goSub").textContent = MODE_TEXT[state.mode];
  document.querySelector(".cmd-who").style.opacity = state.mode === "host" ? ".45" : "";
}
$$("#modeSeg button").forEach((b) => (b.onclick = () => {
  if (state.status?.active) return toast("Stop first to change mode");
  state.mode = b.dataset.mode;
  state.settings.launchMode = state.mode;
  saveSettings();
  renderMode();
}));

async function refreshProfiles() {
  state.profiles = await invoke("list_profiles");
  const players = state.profiles.filter((p) => !p.headless);
  if (!players.find((p) => p.id === state.profileId)) state.profileId = players[0]?.id || null;
  renderWho();
  renderOperator();
}

function renderWho() {
  const p = state.profiles.find((x) => x.id === state.profileId);
  $("whoAvatar").textContent = p ? initials(p.nickname) : "?";
  $("whoName").textContent = p ? p.nickname : "No profile";
  $("whoSub").textContent = p ? `Lv ${p.level} ${p.side || ""} · ${p.username}` : "Create one in the SPT launcher";
  const players = state.profiles.filter((x) => !x.headless);
  $("whoMenu").innerHTML = players.length
    ? players.map((x) => `<div class="who-item ${x.id === state.profileId ? "sel" : ""}" data-id="${esc(x.id)}">
        <div class="who-avatar">${esc(initials(x.nickname))}</div>
        <div><b>${esc(x.nickname)}</b><small>Lv ${x.level} · ${esc(x.username)}${x.valid ? "" : " · incomplete"}</small></div></div>`).join("")
    : `<div class="who-empty">No player profiles on the host yet.</div>`;
  $$(".who-item").forEach((el) => (el.onclick = () => {
    state.profileId = el.dataset.id;
    state.settings.lastProfile = state.profileId;
    saveSettings();
    $("whoMenu").hidden = true;
    renderWho();
    renderOperator();
  }));
}
$("whoBtn").onclick = (e) => { e.stopPropagation(); $("whoMenu").hidden = !$("whoMenu").hidden; };
document.addEventListener("click", (e) => { if (!e.target.closest(".cmd-who")) $("whoMenu").hidden = true; });

function renderOperator() {
  const p = state.profiles.find((x) => x.id === state.profileId);
  if (!p) {
    $("opName").textContent = "No profile";
    $("opSub").textContent = "Nothing to show yet";
    return;
  }
  const sr = p.raids ? Math.round((p.survived / p.raids) * 100) : null;
  const kd = p.deaths ? (p.kills / p.deaths).toFixed(2) : p.kills ? String(p.kills) : "-";
  $("opLevel").textContent = p.level;
  $("opName").textContent = p.nickname;
  $("opSub").textContent = `${p.edition || "Unknown edition"}${p.prestige ? " · Prestige " + p.prestige : ""}`;
  $("opSide").textContent = (p.side || "-").toUpperCase();
  $("opSide").className = "side-tag " + (p.side || "").toLowerCase();
  $("opSr").textContent = sr === null ? "-" : sr + "%";
  $("opRingFg").style.strokeDashoffset = 97.4 - (97.4 * (sr || 0)) / 100;
  $("opRaids").textContent = `${p.survived} of ${p.raids} raids`;
  $("opLast").textContent = "Last raid " + ago(p.lastSession);
  $("opKd").textContent = kd;
  $("opKills").textContent = human(p.kills);
  $("opPmc").textContent = human(p.pmcKills);
  $("opBoss").textContent = human(p.bossKills);
  $("opHs").textContent = human(p.headshots);
  $("opStreak").textContent = p.winStreak + (p.longestStreak ? ` / ${p.longestStreak}` : "");
  $("opRub").textContent = "₽ " + human(p.roubles);
  $("opTime").textContent = p.lifetimeSecs ? dur(p.lifetimeSecs).split(" ")[0] : "-";
  $("opSaved").textContent = "Saved by server " + dur(p.savedAgo).split(" ")[0] + " ago";
  $("opInvalid").hidden = p.valid;
}

// ============================ launch ============================

$("goBtn").onclick = async () => {
  const s = state.status;
  if (s?.active) {
    if (!(await confirmBox("Stop everything?", "Closes the game, headless and server that were started from here.", "Stop"))) return;
    $("goBtn").disabled = true;
    await call("stop").catch(() => {});
    $("goBtn").disabled = false;
    return;
  }
  const p = state.profiles.find((x) => x.id === state.profileId);
  if (state.mode !== "host") {
    if (!p) return toast("Pick who you're playing as", true);
    if (!p.valid && !(await confirmBox("Profile looks incomplete", `${p.nickname} is missing parts of its save. The game may refuse it. Launch anyway?`, "Launch anyway"))) return;
  }
  if (state.mismatch?.length && state.settings.warnMismatch && state.mode !== "host") {
    if (!(await confirmBox("Versions don't match", state.mismatch.join("\n") + "\n\nMismatched installs usually fail to connect.", "Launch anyway"))) return;
  }
  await call("launch", { mode: state.mode, profile: state.mode === "host" ? null : state.profileId }).catch(() => {});
  pollStatus();
};

$("btnRestartHl").onclick = async () => { await call("restart_headless").catch(() => {}); toast("Restarting headless"); };
$("btnStartGame").onclick = async () => {
  if (!state.profileId) return toast("Pick who you're playing as", true);
  await call("start_game", { profile: state.profileId }).catch(() => {});
};

// ============================ status ============================

async function pollStatus() {
  let s;
  try { s = await invoke("status"); } catch { return; }
  state.status = s;

  for (const [key, node] of [["server", "nodeServer"], ["headless", "nodeHeadless"], ["game", "nodeGame"]]) {
    const p = s[key];
    const h = state.hist[key];
    h.push(p.running ? p.ram : 0);
    if (h.length > 60) h.shift();
    paintNode($(node), p, key, s);
    drawSpark($(node).querySelector(".spark"), h);
  }

  const serverUp = s.server.running && s.portOpen;
  $("link1").classList.toggle("on", serverUp && s.headless.running);
  $("link2").classList.toggle("on", serverUp && s.game.running);
  $("srvPort").textContent = ":" + s.port;
  $("hlRestarts").textContent = s.restarts + (s.restarts === 1 ? " restart" : " restarts");
  $("hlRestarts").style.color = s.restarts ? "var(--warn)" : "";
  $("gameTag").textContent = state.boot?.versions.sptPlayer ? "SPT " + state.boot.versions.sptPlayer : "-";
  $("btnRestartHl").disabled = !s.headless.owned;
  $("btnStartGame").disabled = !s.active || s.game.running || !state.boot?.playerDir;

  // while running, the selector shows what is actually running
  const shown = s.active && s.mode ? s.mode : state.mode;
  $$("#modeSeg button").forEach((b) => b.classList.toggle("on", b.dataset.mode === shown));

  // launch button
  const go = $("goBtn");
  const busy = s.active && s.progress < 100;
  go.classList.toggle("running", s.active);
  go.classList.toggle("busy", busy);
  $("goLabel").textContent = s.active ? "STOP" : "LAUNCH";
  $("goSub").textContent = s.active ? (busy ? s.stage + "…" : MODE_TEXT[s.mode] + " running") : MODE_TEXT[state.mode];
  $("goFill").style.width = s.active ? s.progress + "%" : "0";
  $("whoBtn").classList.toggle("locked", s.active);

  // titlebar pill
  const pill = $("stackPill");
  const ext = !s.active && (s.server.running || s.headless.running || s.game.running);
  pill.className = "stack-pill " + (busy ? "busy" : s.active ? "live" : ext ? "ext" : "");
  $("stackPillText").textContent = busy ? s.stage : s.active ? "Live" : ext ? "Running elsewhere" : "Idle";

  $$(".tab").forEach((t) => t.classList.toggle("live", s[STREAM_NAMES[+t.dataset.stream]].running));
}

function paintNode(el, p, key, s) {
  const starting = s.active && !p.running && s.progress < 100 &&
    ((key === "server" && s.mode !== "join") || (key === "headless" && s.mode !== "join" && s.progress >= 65) || (key === "game" && s.mode !== "host" && s.progress >= 85));
  const serverWaiting = key === "server" && p.running && !s.portOpen;
  el.classList.toggle("on", p.running && p.owned && !serverWaiting);
  el.classList.toggle("ext", p.running && !p.owned);
  el.classList.toggle("wait", starting || serverWaiting);
  el.classList.toggle("off", !p.running && !starting);
  el.querySelector(".node-state").textContent =
    serverWaiting ? "Booting…" : starting ? "Starting…" : p.running ? (p.owned ? "Online" : "Running elsewhere") : "Offline";
  el.querySelector(".v-up").textContent = p.running ? dur(p.uptime) : "-";
  el.querySelector(".v-cpu").textContent = p.running ? Math.round(p.cpu) + "%" : "-";
  el.querySelector(".v-ram").textContent = p.running ? bytes(p.ram) : "-";
}

function drawSpark(svg, raw) {
  const n = 60;
  // pad with the oldest sample so a fresh process draws a flat line, not a stub
  const data = Array(n - raw.length).fill(raw[0] || 0).concat(raw);
  const max = Math.max(...data, 1);
  const min = Math.min(...data.filter((v) => v > 0), max) * 0.85;
  const scale = (v) => (v <= 0 ? 32 : 30 - ((v - min) / Math.max(max - min, 1)) * 24);
  const pts = data.map((v, i) => [(i / (n - 1)) * 120, scale(v)]);
  const line = pts.map((p, i) => (i ? "L" : "M") + p[0].toFixed(1) + " " + p[1].toFixed(1)).join(" ");
  svg.querySelector(".spark-line").setAttribute("d", line);
  svg.querySelector(".spark-area").setAttribute("d", line + ` L120 34 L${pts[0][0].toFixed(1)} 34 Z`);
}

// ============================ lobby ============================

async function pollLobby() {
  if (!state.status?.portOpen) return renderLobby(null);
  try { renderLobby(await invoke("lobby", { session: state.profileId })); } catch { renderLobby(null); }
}

function renderLobby(l) {
  const raids = l?.raids || [];
  const players = l?.players || [];
  $("fikaSrvVer").textContent = l?.online ? "Fika server " + (l.version || "?") : "server offline";
  $("raidCount").textContent = raids.length;
  $("onlineCount").textContent = players.length;

  $("raidList").innerHTML = raids.length
    ? raids.map((r) => {
        const cls = r.status === 0 ? "loading" : r.status === 2 ? "done" : "";
        const st = ["Loading", "In raid", "Finished"][r.status] || "Unknown";
        return `<div class="raid ${cls}">
          <div class="raid-count">${r.players}<small>${r.players === 1 ? "player" : "players"}</small></div>
          <b>${esc(mapName(r.location))}</b>
          <div class="raid-meta">Started by ${esc(r.host || "?")}</div>
          <div class="raid-tags"><span class="tag ${r.status === 1 ? "live" : ""}">${st}</span><span class="tag">${r.side === 1 ? "Scav" : "PMC"}</span>${r.headless ? '<span class="tag hl">Headless hosted</span>' : ""}</div>
        </div>`;
      }).join("")
    : `<div class="empty"><svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></svg>${l?.online ? "No raids running." : "Start the server to see raids."}</div>`;

  $("playerList").innerHTML = players.length
    ? players.map((p) => {
        const inRaid = p.activity === 1;
        const where = inRaid && p.location ? " · " + mapName(p.location) : "";
        const since = p.since ? " · " + dur(Math.max(0, Math.floor(Date.now() / 1000 - p.since))).split(" ")[0] : "";
        return `<div class="player"><div class="who-avatar">${esc(initials(p.nickname))}</div>
          <div class="p-name"><b>${esc(p.nickname)}</b><small>Level ${p.level}</small></div>
          <span class="act ${inRaid ? "raid" : ""}">${ACTIVITY[p.activity] || "Unknown"}${esc(where)}${since}</span></div>`;
      }).join("")
    : `<div class="empty"><svg viewBox="0 0 24 24"><circle cx="9" cy="8" r="3.5"/><path d="M3 20c0-3.3 2.7-6 6-6s6 2.7 6 6M16 4.5a3.5 3.5 0 0 1 0 7M21 20c0-2.6-1.6-4.8-4-5.6"/></svg>${l?.online ? "Nobody online yet." : "Nobody online."}</div>`;
}

// ============================ console ============================

const ERR_RE = /\b(error|exception|fatal|failed|failure)\b|\[error/i;
const WARN_RE = /\b(warn|warning)\b|\[warning/i;
const OK_RE = /\b(started|listening|ready|success|loaded)\b/i;

function levelOf(line) {
  if (line.startsWith("[Fika Control]")) return "mine";
  if (ERR_RE.test(line)) return "error";
  if (WARN_RE.test(line)) return "warn";
  if (OK_RE.test(line)) return "ok";
  return "";
}

async function pollLogs() {
  for (let i = 0; i < 3; i++) {
    let c;
    try { c = await invoke("logs_since", { stream: i, after: state.logSeq[i] }); } catch { continue; }
    if (c.reset) { state.logs[i] = []; state.logErr[i] = 0; state.logSeen[i] = 0; if (i === state.stream) renderLog(true); }
    if (!c.lines.length) continue;
    const added = c.lines.map(([seq, text]) => ({ text, lvl: levelOf(text) }));
    state.logSeq[i] = c.lines[c.lines.length - 1][0];
    state.logErr[i] += added.filter((l) => l.lvl === "error").length;
    state.logs[i].push(...added);
    if (state.logs[i].length > 6000) state.logs[i].splice(0, state.logs[i].length - 6000);
    if (i === state.stream && state.page === "console") appendLog(added);
  }
  if (state.page === "console") state.logSeen[state.stream] = state.logErr[state.stream];
  updateBadges();
}

function updateBadges() {
  let total = 0;
  $$(".tab").forEach((t) => {
    const i = +t.dataset.stream;
    const n = state.logErr[i] - state.logSeen[i];
    const badge = t.querySelector(".tab-err");
    badge.hidden = state.logErr[i] === 0;
    badge.textContent = state.logErr[i] > 999 ? "999+" : state.logErr[i];
    if (state.page !== "console" || i !== state.stream) total += Math.max(0, n);
  });
  $("badgeConsole").hidden = total === 0;
  $("badgeConsole").textContent = total > 99 ? "99+" : total;
}

function visible(l) {
  if (state.level === "error" && l.lvl !== "error") return false;
  if (state.level === "warn" && l.lvl !== "error" && l.lvl !== "warn") return false;
  const q = $("logSearch").value.trim().toLowerCase();
  return !q || l.text.toLowerCase().includes(q);
}

function lineHtml(l) {
  let t = esc(l.text);
  const q = $("logSearch").value.trim();
  if (q) t = t.replace(new RegExp(q.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "gi"), (m) => `<mark>${m}</mark>`);
  t = t.replace(/^(\[?\d{2}:\d{2}:\d{2}(?:[.,]\d+)?\]?|\[\d{4}-\d{2}-\d{2}[^\]]*\])/, '<span class="ts">$1</span>');
  return `<div class="ln ${l.lvl}">${t || " "}</div>`;
}

function renderLog(scrollEnd) {
  const view = $("logView");
  const lines = state.logs[state.stream].filter(visible).slice(-4000);
  view.innerHTML = lines.length
    ? lines.map(lineHtml).join("")
    : `<div class="log-empty">${state.logs[state.stream].length ? "No lines match." : emptyText()}</div>`;
  $("logStats").textContent = `${lines.length} of ${state.logs[state.stream].length} lines`;
  if (scrollEnd || $("logFollow").checked) view.scrollTop = view.scrollHeight;
}

function emptyText() {
  return ["Server output shows up here once it's launched from Fika Control.",
          "Headless BepInEx log. Waiting for new lines.",
          "Your game's BepInEx log. Waiting for new lines."][state.stream];
}

function appendLog(added) {
  const view = $("logView");
  const show = added.filter(visible);
  if (!show.length) return;
  if (view.querySelector(".log-empty")) view.innerHTML = "";
  const atEnd = view.scrollHeight - view.scrollTop - view.clientHeight < 40;
  view.insertAdjacentHTML("beforeend", show.map(lineHtml).join(""));
  while (view.childElementCount > 4000) view.firstElementChild.remove();
  $("logStats").textContent = `${view.childElementCount} of ${state.logs[state.stream].length} lines`;
  if ($("logFollow").checked && atEnd) view.scrollTop = view.scrollHeight;
  else if (!atEnd) $("logJump").hidden = false;
}

function setStream(i) {
  state.stream = i;
  $$(".tab").forEach((t) => t.classList.toggle("active", +t.dataset.stream === i));
  state.logSeen[i] = state.logErr[i];
  renderLog(true);
  updateBadges();
}
$$(".tab").forEach((t) => (t.onclick = () => setStream(+t.dataset.stream)));
$$("#levelSeg button").forEach((b) => (b.onclick = () => {
  state.level = b.dataset.level;
  $$("#levelSeg button").forEach((x) => x.classList.toggle("on", x === b));
  renderLog(true);
}));
let searchT;
$("logSearch").oninput = () => { clearTimeout(searchT); searchT = setTimeout(() => renderLog(true), 120); };
$("logWrap").onchange = () => $("logView").classList.toggle("wrap", $("logWrap").checked);
$("logView").onscroll = () => {
  const v = $("logView");
  if (v.scrollHeight - v.scrollTop - v.clientHeight < 40) $("logJump").hidden = true;
};
$("logJump").onclick = () => { $("logView").scrollTop = $("logView").scrollHeight; $("logJump").hidden = true; };
const logText = () => state.logs[state.stream].filter(visible).map((l) => l.text).join("\n");
$("logCopy").onclick = async () => {
  const t = logText();
  if (!t) return toast("Nothing to copy");
  try { await navigator.clipboard.writeText(t); toast("Copied"); } catch { toast("Clipboard unavailable", true); }
};
$("logSave").onclick = async () => {
  const t = logText();
  if (!t) return toast("Nothing to save");
  const stamp = new Date().toISOString().slice(0, 19).replace(/[-:T]/g, "");
  if (await call("save_text", { name: `${STREAM_NAMES[state.stream]}-${stamp}.log`, text: t }).catch(() => false)) toast("Saved");
};
$("logClear").onclick = () => {
  invoke("clear_logs", { stream: state.stream });
  state.logs[state.stream] = [];
  state.logErr[state.stream] = state.logSeen[state.stream] = 0;
  renderLog(true);
  updateBadges();
};
$("logOpen").onclick = () => call("open_folder", { which: ["host", "headless-log", "game-log"][state.stream] }).catch(() => {});

// ============================ configs ============================

async function loadConfigRoots() {
  const roots = await invoke("config_roots");
  $("cfgRoots").innerHTML = roots.map((r) => `<button data-root="${r.key}" ${r.exists ? "" : "disabled"} title="${esc(r.path)}">${esc(r.label)}</button>`).join("");
  $$("#cfgRoots button").forEach((b) => (b.onclick = () => selectRoot(b.dataset.root)));
  const first = roots.find((r) => r.exists && r.key === state.cfg.root) || roots.find((r) => r.exists);
  if (first) selectRoot(first.key);
}

async function selectRoot(key) {
  if (!(await leaveDirty())) return;
  state.cfg.root = key;
  $$("#cfgRoots button").forEach((b) => b.classList.toggle("on", b.dataset.root === key));
  $("cfgList").innerHTML = `<div class="empty">Scanning…</div>`;
  state.cfg.entries = await invoke("config_list", { root: key, all: $("cfgAll").checked }).catch(() => []);
  renderCfgList();
}

function renderCfgList() {
  const q = $("cfgSearch").value.trim().toLowerCase();
  const list = state.cfg.entries.filter((e) => !q || (e.group + " " + e.rel).toLowerCase().includes(q));
  const groups = {};
  for (const e of list) (groups[e.group] ||= []).push(e);
  $("cfgList").innerHTML = list.length
    ? Object.entries(groups).map(([g, items]) => `<div class="cfg-group"><b title="${esc(g)}">${esc(g)}</b>${items.map((e) => {
        const ext = e.name.split(".").pop();
        const sel = state.cfg.open === e.rel ? "sel" : "";
        const label = e.rel.includes("\\") ? e.rel.split("\\").slice(1).join("\\") : e.name;
        return `<div class="cfg-item ${sel}" data-rel="${esc(e.rel)}" title="${esc(e.rel)}"><span>${esc(label)}</span><i>${esc(ext)}</i></div>`;
      }).join("")}</div>`).join("")
    : `<div class="empty">${state.cfg.entries.length ? "No match." : "No config files found."}</div>`;
  $$(".cfg-item").forEach((el) => (el.onclick = () => openConfig(el.dataset.rel)));
}
$("cfgSearch").oninput = renderCfgList;
$("cfgAll").onchange = () => state.cfg.root && selectRoot(state.cfg.root);

const isDirty = () => state.cfg.open && $("cfgText").value !== state.cfg.original;
async function leaveDirty() {
  if (!isDirty()) return true;
  return confirmBox("Discard changes?", `You have unsaved edits in ${state.cfg.open.split("\\").pop()}.`, "Discard");
}

async function openConfig(rel) {
  if (state.cfg.open !== rel && !(await leaveDirty())) return;
  let doc;
  try { doc = await invoke("config_read", { root: state.cfg.root, rel }); }
  catch (e) { return cfgStatus("bad", String(e)); }
  state.cfg.open = rel;
  state.cfg.stamp = doc.stamp;
  state.cfg.original = doc.text;
  const ta = $("cfgText");
  ta.value = doc.text;
  ta.disabled = !!doc.readOnly;
  ta.scrollTop = 0;
  $("cfgName").textContent = rel.split("\\").pop();
  $("cfgPath").textContent = rel;
  $("cfgOpenDir").disabled = false;
  updateGutter();
  renderCfgList();
  if (doc.readOnly) cfgStatus("bad", doc.readOnly);
  else validateCfg();
  updateCfgButtons();
}

function cfgStatus(kind, text) {
  $("cfgStatus").className = "cfg-status " + kind;
  $("cfgStatusText").textContent = text;
}
function updateCfgButtons() {
  const d = isDirty();
  $("cfgSave").disabled = !d;
  $("cfgRevert").disabled = !d;
}
function updateGutter() {
  const n = $("cfgText").value.split("\n").length;
  let s = "";
  for (let i = 1; i <= n; i++) s += i + "\n";
  $("cfgGutter").textContent = s;
}
let valT;
async function validateCfg() {
  if (!state.cfg.open) return;
  try {
    const msg = await invoke("config_validate", { rel: state.cfg.open, text: $("cfgText").value });
    cfgStatus(isDirty() ? "dirty" : "ok", (isDirty() ? "Unsaved · " : "") + msg);
  } catch (e) { cfgStatus("bad", String(e)); }
}
$("cfgText").oninput = () => {
  updateGutter();
  updateCfgButtons();
  clearTimeout(valT);
  valT = setTimeout(validateCfg, 250);
};
$("cfgText").onscroll = () => ($("cfgGutter").scrollTop = $("cfgText").scrollTop);
$("cfgText").onkeydown = (e) => {
  if (e.key === "Tab") {
    e.preventDefault();
    const ta = e.target, s = ta.selectionStart;
    ta.setRangeText("    ", s, ta.selectionEnd, "end");
    ta.oninput();
  }
};
$("cfgRevert").onclick = () => { $("cfgText").value = state.cfg.original; $("cfgText").oninput(); };
$("cfgOpenDir").onclick = () => call("open_folder", { which: state.cfg.root + "|" + state.cfg.open }).catch(() => {});
async function saveConfig() {
  if (!isDirty()) return;
  try {
    state.cfg.stamp = await invoke("config_save", { root: state.cfg.root, rel: state.cfg.open, text: $("cfgText").value, stamp: state.cfg.stamp });
    state.cfg.original = $("cfgText").value;
    updateCfgButtons();
    validateCfg();
    toast("Saved, backup kept");
  } catch (e) { cfgStatus("bad", String(e)); toast("Not saved", true); }
}
$("cfgSave").onclick = saveConfig;

boot();
