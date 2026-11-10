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
  runs: [[], [], []],
  runView: ["current", "current", "current"],
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
  if (owns && !(await confirmBox("Quit Fika Hub?", "This stops the server, headless and game that were started from here.", "Stop and quit"))) return;
  invoke("quit");
};

function goto(page) {
  state.page = page;
  $$(".rail-btn").forEach((b) => b.classList.toggle("active", b.dataset.page === page));
  moveRailInd();
  $$(".page").forEach((p) => p.classList.toggle("active", p.id === "page-" + page));
  if (page === "console") { state.logSeen = [...state.logErr]; renderRuns(); renderLog(true); updateBadges(); }
  if (page === "configs" && !state.cfg.root) loadConfigRoots();
  if (page === "settings") { loadConnection(); loadNextRestart(); }
  if (page === "history") loadHistory();
}
$$(".rail-btn").forEach((b) => (b.onclick = () => goto(b.dataset.page)));

// one indicator that slides between buttons instead of each button drawing its own
function moveRailInd() {
  const b = document.querySelector(".rail-btn.active");
  const ind = $("railInd");
  if (!b) return;
  ind.style.transform = `translateY(${b.offsetTop + 12}px)`;
  ind.style.height = b.offsetHeight - 24 + "px";
}
window.addEventListener("resize", moveRailInd);
requestAnimationFrame(moveRailInd);

function setPill(cls, text, meta, progress) {
  const pill = $("stackPill");
  const t = $("stackPillText");
  pill.className = "stack-pill " + cls;
  if (t.textContent !== text) {
    t.textContent = text;
    // restart the slide-in so every state change reads as one
    t.classList.remove("swap");
    void t.offsetWidth;
    t.classList.add("swap");
  }
  $("stackPillMeta").hidden = !meta;
  $("stackPillMeta").textContent = meta || "";
  $("stackPillBar").style.width = progress == null ? "0" : progress + "%";
}
$$("[data-open]").forEach((b) => (b.onclick = () => call("open_folder", { which: b.dataset.open }).catch(() => {})));
$$("[data-goto-log]").forEach((b) => (b.onclick = () => { setStream(+b.dataset.gotoLog); goto("console"); }));

document.addEventListener("keydown", (e) => {
  if (e.ctrlKey && e.key.toLowerCase() === "s" && state.page === "configs") { e.preventDefault(); saveConfig(); }
  if (e.ctrlKey && e.key.toLowerCase() === "f" && state.page === "console") { e.preventDefault(); $("logSearch").focus(); }
  if (e.key === "Escape") $("whoMenu").hidden = true;
});

// ============================ boot + setup ============================

async function boot() {
  [0, 1, 2].forEach(resetRuns);
  const b = await invoke("boot");
  applyBoot(b);
  state.mode = b.settings.launchMode || "full";
  state.profileId = b.settings.lastProfile;
  renderMode();
  if (b.hostProblem) showSetup();
  await refreshProfiles();
  loadConnection();
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
  applySettingValues();
  $$("#displaySeg button").forEach((x) => x.classList.toggle("on", x.dataset.display === (b.settings.displayMode || "")));
  $("aboutText").textContent = `Fika Hub ${b.appVersion} · by 20fpsguy`;
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

function saveSettings() { return invoke("save_settings", { settings: state.settings }); }
$$("[data-set]").forEach((i) => (i.onchange = () => { state.settings[i.dataset.set] = i.checked; saveSettings(); }));
$$("#displaySeg button").forEach((b) => (b.onclick = () => {
  state.settings.displayMode = b.dataset.display;
  $$("#displaySeg button").forEach((x) => x.classList.toggle("on", x === b));
  saveSettings();
  toast(b.dataset.display ? `Game will start ${b.textContent.toLowerCase()}` : "Window mode left alone");
}));

// ============================ connection ============================

const LOOPBACK = (ip) => !ip || ip === "localhost" || ip.startsWith("127.");

async function loadConnection() {
  const c = await invoke("connection_info").catch(() => null);
  if (!c) return;
  state.conn = c;
  const lan = c.lanIp ? `${c.lanIp}:${c.port}` : null;
  $("connLan").textContent = lan || "No network found";
  $("connCopy").disabled = !lan;
  $("srvPort").title = lan ? `Click to copy ${lan} for friends` : "";
  $("serverAddr").placeholder = `127.0.0.1:${c.port}`;
  if (document.activeElement !== $("serverAddr")) $("serverAddr").value = state.settings.serverAddress || "";
  $("serverAddrNote").textContent = state.settings.serverAddress ? `Game only mode connects to ${c.gameAddr}.` : "Game only mode connects to this PC.";
}

async function copyLan() {
  const c = state.conn;
  if (!c?.lanIp) return;
  try { await navigator.clipboard.writeText(`${c.lanIp}:${c.port}`); toast("Address copied"); } catch { toast("Clipboard unavailable", true); }
}
$("connCopy").onclick = copyLan;
$("srvPort").onclick = copyLan;

function checkRow(kind, text) {
  return `<div class="check ${kind}"><span class="dot"></span><span>${esc(text)}</span></div>`;
}

$("connCheck").onclick = async () => {
  await loadConnection();
  const c = state.conn;
  if (!c) return;
  const rows = [];
  if (LOOPBACK(c.bindIp)) rows.push(checkRow("bad", `SPT only listens on this PC (ip is ${c.bindIp || "unset"} in http.json). Set it to 0.0.0.0 so friends can reach it.`));
  else rows.push(checkRow("ok", `SPT listens on ${c.bindIp}.`));
  if (LOOPBACK(c.backendIp)) rows.push(checkRow("warn", `backendIp is ${c.backendIp || "unset"}, which sends friends back to their own PC. Set it to ${c.lanIp || "your LAN IP"}, or your public IP for internet play.`));
  else rows.push(checkRow("ok", `backendIp is ${c.backendIp}.`));
  if (!c.lanIp) rows.push(checkRow("bad", "No LAN address found. Is this PC on a network?"));
  else if (!state.status?.portOpen) rows.push(checkRow("warn", "Start the server to test the port."));
  else {
    $("connCheck").disabled = true;
    const ok = await invoke("check_reach", { addr: `${c.lanIp}:${c.port}` }).catch(() => false);
    $("connCheck").disabled = false;
    rows.push(ok
      ? checkRow("ok", `Port ${c.port} answers on ${c.lanIp}. Windows Firewall can still block other PCs, this test runs from here.`)
      : checkRow("bad", `Port ${c.port} doesn't answer on ${c.lanIp}. Check the ip above, then restart the server.`));
  }
  $("connChecks").innerHTML = rows.join("");
};

$("serverAddr").onchange = async () => {
  state.settings.serverAddress = $("serverAddr").value.trim();
  await saveSettings();
  loadConnection();
};
$("serverAddrTest").onclick = async () => {
  await $("serverAddr").onchange();
  const addr = state.conn?.gameAddr;
  if (!addr) return;
  $("serverAddrTest").disabled = true;
  const ok = await invoke("check_reach", { addr }).catch(() => false);
  $("serverAddrTest").disabled = false;
  toast(ok ? `${addr} is reachable` : `Nothing answers at ${addr}`, !ok);
};

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
  if (state.mode !== "host" && !p) return toast("Pick who you're playing as", true);
  if (state.settings.checkBeforeLaunch) {
    if (!(await preflight())) return;
  } else if (state.mode !== "host") {
    if (!p.valid && !(await confirmBox("Profile looks incomplete", `${p.nickname} is missing parts of its save. The game may refuse it. Launch anyway?`, "Launch anyway"))) return;
  }
  if (!state.settings.checkBeforeLaunch && state.mismatch?.length && state.settings.warnMismatch && state.mode !== "host") {
    if (!(await confirmBox("Versions don't match", state.mismatch.join("\n") + "\n\nMismatched installs usually fail to connect.", "Launch anyway"))) return;
  }
  await call("launch", { mode: state.mode, profile: state.mode === "host" ? null : state.profileId }).catch(() => {});
  pollStatus();
};

const hlRaids = () => (state.lobby?.raids || []).filter((r) => r.headless && r.status !== 2).length;
const plural = (n, w) => `${n} ${w}${n === 1 ? "" : "s"}`;

$("btnRestartHl").onclick = async () => {
  const n = hlRaids();
  if (n && !(await confirmBox("Restart headless?", `${plural(n, "raid")} running on the headless will end for everyone in them. The server and connected players stay up.`, "Restart headless"))) return;
  await call("restart_headless").catch(() => {});
  toast("Restarting headless");
};
$("btnRestartSrv").onclick = async () => {
  const players = state.lobby?.players?.length || 0;
  const who = players ? `${plural(players, "connected player")} (your game included) will be disconnected` : "Anyone connected, your game included, will be disconnected";
  const body = `${who}. The headless goes down too and comes back once the server is up again, so raids it hosts will end.`;
  if (!(await confirmBox("Restart the server?", body, "Restart server"))) return;
  state.expectSrvExit = true;
  await call("restart_server").catch(() => {});
};
$("btnStartGame").onclick = async () => {
  if (!state.profileId) return toast("Pick who you're playing as", true);
  await call("start_game", { profile: state.profileId }).catch(() => {});
};

// ============================ status ============================

async function pollStatus() {
  let s;
  try { s = await invoke("status"); } catch { return; }
  const prev = state.status;
  state.status = s;
  // a stack we started lost its server without being told to
  if (prev?.active && s.active && prev.server.owned && prev.server.running && !s.server.running && !state.expectSrvExit) {
    alertUser("crash", "SPT server stopped", "The server started from Fika Hub is no longer running.");
  }
  if (s.server.running || !s.active) state.expectSrvExit = false;

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
  $("btnRestartSrv").disabled = !s.server.owned || s.progress < 100;
  paintHeadlessPower(s);
  if (state.page === "history") paintBkBar();
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
  const ext = !s.active && (s.server.running || s.headless.running || s.game.running);
  const up = Math.max(s.server.uptime || 0, s.headless.uptime || 0, s.game.uptime || 0);
  if (busy) setPill("busy", s.stage, s.progress + "%", s.progress);
  else if (s.active) setPill("live", "Live", `${MODE_TEXT[s.mode] || ""} · ${dur(up)}`, null);
  else if (ext) setPill("ext", "Running elsewhere", null, null);
  else setPill("", "Idle", null, null);
  document.body.classList.toggle("is-live", s.active && !busy);
  document.body.classList.toggle("is-busy", busy);

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
  state.lobby = l;
  watchPlayers(l);
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

// ============================ notifications ============================

// only when the window isn't in front, otherwise the dashboard already says it
function alertUser(kind, title, body) {
  const on = kind === "crash" ? state.settings?.notifyCrash : state.settings?.notifyPlayers;
  if (!on || document.hasFocus()) return;
  invoke("notify", { title, body }).catch(() => {});
}

function watchLine(stream, text) {
  if (stream !== 1 || !text.startsWith("[Fika Hub] Headless exited")) return;
  const auto = text.includes("restarting");
  if (auto) state.hlCrashed = true;
  alertUser("crash", "Headless crashed", auto ? "Restarting it in 3 seconds." : "Auto-restart is off, so it stays down.");
}

function watchPlayers(l) {
  if (!l?.online) { state.seenPlayers = null; return; }
  const me = state.profiles.find((p) => p.id === state.profileId)?.nickname;
  const now = new Set((l.players || []).map((p) => p.nickname).filter((n) => n && n !== me));
  // the first list after the server comes up is a baseline, not a wave of joins
  if (state.seenPlayers) {
    const joined = [...now].filter((n) => !state.seenPlayers.has(n));
    const left = [...state.seenPlayers].filter((n) => !now.has(n));
    if (joined.length) alertUser("players", joined.length === 1 ? `${joined[0]} connected` : `${joined.length} players connected`, joined.join(", "));
    if (left.length) alertUser("players", left.length === 1 ? `${left[0]} disconnected` : `${left.length} players disconnected`, left.join(", "));
  }
  state.seenPlayers = now;
}

// ============================ console ============================

const ERR_RE = /\b(error|exception|fatal|failed|failure)\b|\[error/i;
const WARN_RE = /\b(warn|warning)\b|\[warning/i;
const OK_RE = /\b(started|listening|ready|success|loaded)\b/i;
const ROUTINE_RE = /\[(Client|WebSocket) Request\]/;
const RUN_MARK = "[Fika Hub] New run: ";
const CONTEXT = 3;

function levelOf(line) {
  if (line.startsWith("[Fika Hub]")) return "mine";
  if (ERR_RE.test(line)) return "error";
  if (WARN_RE.test(line)) return "warn";
  if (OK_RE.test(line)) return "ok";
  return "";
}

function resetRuns(i) {
  state.runs[i] = [{ id: 0, label: "Before launch", time: null }];
  state.runView[i] = "current";
}
const curRun = (i) => state.runs[i][state.runs[i].length - 1].id;

async function pollLogs() {
  for (let i = 0; i < 3; i++) {
    let c;
    try { c = await invoke("logs_since", { stream: i, after: state.logSeq[i] }); } catch { continue; }
    if (c.reset) {
      state.logs[i] = []; state.logErr[i] = 0; state.logSeen[i] = 0; resetRuns(i);
      if (i === state.stream && state.page === "console") { renderRuns(); renderLog(true); }
    }
    if (!c.lines.length) continue;
    state.logSeq[i] = c.lines[c.lines.length - 1][0];
    let newRun = false;
    const added = c.lines.map(([, text]) => {
      if (text.startsWith(RUN_MARK)) {
        // each run gets its own error count, so a restarted headless starts clean
        state.runs[i].push({ id: curRun(i) + 1, label: text.slice(RUN_MARK.length), time: new Date() });
        state.logErr[i] = state.logSeen[i] = 0;
        newRun = true;
      }
      const l = { text, lvl: levelOf(text), run: curRun(i) };
      if (l.lvl === "error") state.logErr[i]++;
      watchLine(i, text);
      return l;
    });
    state.logs[i].push(...added);
    if (state.logs[i].length > 6000) state.logs[i].splice(0, state.logs[i].length - 6000);
    if (i === state.stream && state.page === "console") {
      if (newRun) renderRuns();
      if (newRun || state.level === "context") renderLog(false);
      else appendLog(added);
    }
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
    badge.title = "Errors in the current run";
    if (state.page !== "console" || i !== state.stream) total += Math.max(0, n);
  });
  $("badgeConsole").hidden = total === 0;
  $("badgeConsole").textContent = total > 99 ? "99+" : total;
}

function inRun(l) {
  const v = state.runView[state.stream];
  if (v === "all") return true;
  return l.run === (v === "current" ? curRun(state.stream) : +v);
}

const quiet = (l) => $("logQuiet").checked && ROUTINE_RE.test(l.text);

function visible(l) {
  if (!inRun(l) || quiet(l)) return false;
  if (state.level === "error" && l.lvl !== "error") return false;
  if (state.level === "warn" && l.lvl !== "error" && l.lvl !== "warn") return false;
  const q = $("logSearch").value.trim().toLowerCase();
  return !q || l.text.toLowerCase().includes(q);
}

// "Errors + context" needs neighbours, so it filters the whole list instead of line by line
function shownLines() {
  const all = state.logs[state.stream];
  if (state.level !== "context") return all.filter(visible);
  const base = all.filter((l) => inRun(l) && !quiet(l));
  const q = $("logSearch").value.trim().toLowerCase();
  const keep = new Set();
  base.forEach((l, i) => {
    if (l.lvl === "error" && (!q || l.text.toLowerCase().includes(q))) for (let k = i - CONTEXT; k <= i + CONTEXT; k++) keep.add(k);
  });
  const out = [];
  let last = -1;
  for (const i of [...keep].sort((x, y) => x - y)) {
    if (i < 0 || i >= base.length) continue;
    if (last >= 0 && i > last + 1) out.push({ text: "", lvl: "gap" });
    out.push(base[i]);
    last = i;
  }
  return out;
}

function lineHtml(l) {
  if (l.lvl === "gap") return `<div class="ln gap"></div>`;
  let t = esc(l.text);
  const q = $("logSearch").value.trim();
  if (q) t = t.replace(new RegExp(q.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "gi"), (m) => `<mark>${m}</mark>`);
  t = t.replace(/^(\[?\d{2}:\d{2}:\d{2}(?:[.,]\d+)?\]?|\[\d{4}-\d{2}-\d{2}[^\]]*\])/, '<span class="ts">$1</span>');
  const cls = l.text.startsWith(RUN_MARK) ? "mine run" : l.lvl;
  return `<div class="ln ${cls}">${t || " "}</div>`;
}

function renderLog(scrollEnd) {
  const view = $("logView");
  const lines = shownLines().slice(-4000);
  const total = state.logs[state.stream].filter(inRun).length;
  view.innerHTML = lines.length
    ? lines.map(lineHtml).join("")
    : `<div class="log-empty">${total ? "No lines match." : emptyText()}</div>`;
  $("logStats").textContent = `${lines.filter((l) => l.lvl !== "gap").length} of ${total} lines`;
  if (scrollEnd || $("logFollow").checked) view.scrollTop = view.scrollHeight;
}

function emptyText() {
  if (!["current", "all"].includes(state.runView[state.stream])) return "Nothing logged in that run.";
  return ["Server output shows up here once it's launched from Fika Hub.",
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
  $("logStats").textContent = `${view.childElementCount} of ${state.logs[state.stream].filter(inRun).length} lines`;
  if ($("logFollow").checked && atEnd) view.scrollTop = view.scrollHeight;
  else if (!atEnd) $("logJump").hidden = false;
}

function renderRuns() {
  const i = state.stream;
  const hhmm = (d) => d.toTimeString().slice(0, 5);
  const opts = [`<option value="current">Current run</option>`];
  // newest first; the current run is already the top option
  for (const r of state.runs[i].slice(0, -1).reverse()) {
    if (!state.logs[i].some((l) => l.run === r.id)) continue;
    opts.push(`<option value="${r.id}">${r.time ? `Run ${r.id} · ${hhmm(r.time)}` : esc(r.label)}</option>`);
  }
  opts.push(`<option value="all">All runs</option>`);
  const sel = $("logRun");
  sel.innerHTML = opts.join("");
  sel.value = [...sel.options].some((o) => o.value === String(state.runView[i])) ? String(state.runView[i]) : "current";
  state.runView[i] = sel.value;
  sel.disabled = opts.length <= 2;
}

function setStream(i) {
  state.stream = i;
  $$(".tab").forEach((t) => t.classList.toggle("active", +t.dataset.stream === i));
  state.logSeen[i] = state.logErr[i];
  renderRuns();
  renderLog(true);
  updateBadges();
}
$$(".tab").forEach((t) => (t.onclick = () => setStream(+t.dataset.stream)));
$$("#levelSeg button").forEach((b) => (b.onclick = () => {
  state.level = b.dataset.level;
  $$("#levelSeg button").forEach((x) => x.classList.toggle("on", x === b));
  renderLog(true);
}));
$("logRun").onchange = () => { state.runView[state.stream] = $("logRun").value; renderLog(true); };
$("logQuiet").onchange = () => renderLog(true);
let searchT;
$("logSearch").oninput = () => { clearTimeout(searchT); searchT = setTimeout(() => renderLog(true), 120); };
$("logWrap").onchange = () => $("logView").classList.toggle("wrap", $("logWrap").checked);
$("logView").onscroll = () => {
  const v = $("logView");
  if (v.scrollHeight - v.scrollTop - v.clientHeight < 40) $("logJump").hidden = true;
};
$("logJump").onclick = () => { $("logView").scrollTop = $("logView").scrollHeight; $("logJump").hidden = true; };
$("logCopy").onclick = async () => {
  const t = shownLines().filter((l) => l.lvl !== "gap").map((l) => l.text).join("\n");
  if (!t) return toast("Nothing to copy");
  try { await navigator.clipboard.writeText(t); toast("Copied what's shown"); } catch { toast("Clipboard unavailable", true); }
};
$("logSave").onclick = async () => {
  // always the raw stream: filters are for reading, saved logs are for support
  const t = state.logs[state.stream].map((l) => l.text).join("\n");
  if (!t) return toast("Nothing to save");
  const stamp = new Date().toISOString().slice(0, 19).replace(/[-:T]/g, "");
  if (await call("save_text", { name: `${STREAM_NAMES[state.stream]}-${stamp}.log`, text: t }).catch(() => false)) toast("Saved the full log");
};
$("logClear").onclick = () => {
  invoke("clear_logs", { stream: state.stream });
  state.logs[state.stream] = [];
  state.logErr[state.stream] = state.logSeen[state.stream] = 0;
  resetRuns(state.stream);
  renderRuns();
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

const cfgKey = (rel, root = state.cfg.root) => root + "|" + rel;
const isPinned = (rel) => (state.settings.pinnedConfigs || []).includes(cfgKey(rel));

function cfgItem(e, withGroup) {
  const ext = e.name.split(".").pop();
  const sel = state.cfg.open === e.rel && state.cfg.openRoot === state.cfg.root ? "sel" : "";
  const label = e.rel.includes("\\") ? e.rel.split("\\").slice(1).join("\\") : e.name;
  // outside its own group the mod folder is the useful part, so lead with it
  const text = withGroup && e.group ? `<em>${esc(e.group)}</em> ${esc(label)}` : esc(label);
  return `<div class="cfg-item ${sel}" data-rel="${esc(e.rel)}" title="${esc(e.rel)}"><span>${text}</span>${isPinned(e.rel) ? '<b class="pin" title="Pinned">★</b>' : ""}<i>${esc(ext)}</i></div>`;
}

function renderCfgList() {
  const q = $("cfgSearch").value.trim().toLowerCase();
  const list = state.cfg.entries.filter((e) => !q || (e.group + " " + e.rel).toLowerCase().includes(q));
  const byRel = new Map(state.cfg.entries.map((e) => [e.rel, e]));
  const pick = (keys) => keys
    .filter((k) => k.startsWith(state.cfg.root + "|"))
    .map((k) => byRel.get(k.slice(state.cfg.root.length + 1)))
    .filter(Boolean);
  const pinned = q ? [] : pick(state.settings.pinnedConfigs || []);
  const recent = q ? [] : pick(state.settings.recentConfigs || []).filter((e) => !isPinned(e.rel)).slice(0, 5);

  const groups = {};
  for (const e of list) (groups[e.group] ||= []).push(e);
  const head = [["Pinned", pinned], ["Recent", recent]]
    .filter(([, items]) => items.length)
    .map(([g, items]) => `<div class="cfg-group quick-group"><b>${g}</b>${items.map((e) => cfgItem(e, true)).join("")}</div>`)
    .join("");
  $("cfgList").innerHTML = list.length
    ? head + Object.entries(groups).map(([g, items]) => `<div class="cfg-group"><b title="${esc(g)}">${esc(g)}</b>${items.map((e) => cfgItem(e, false)).join("")}</div>`).join("")
    : `<div class="empty">${state.cfg.entries.length ? "No match." : "No config files found."}</div>`;
  $$(".cfg-item").forEach((el) => (el.onclick = () => openConfig(el.dataset.rel)));
}

function renderPin() {
  const open = !!state.cfg.open;
  const on = open && (state.settings.pinnedConfigs || []).includes(cfgKey(state.cfg.open, state.cfg.openRoot));
  $("cfgPin").disabled = !open;
  $("cfgPin").classList.toggle("on", on);
  $("cfgPin").textContent = on ? "★ Pinned" : "☆ Pin";
}
$("cfgPin").onclick = () => {
  if (!state.cfg.open) return;
  const key = cfgKey(state.cfg.open, state.cfg.openRoot);
  const list = state.settings.pinnedConfigs || [];
  state.settings.pinnedConfigs = list.includes(key) ? list.filter((k) => k !== key) : [...list, key];
  saveSettings();
  renderPin();
  renderCfgList();
};
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
  state.cfg.openRoot = state.cfg.root;
  const key = cfgKey(rel);
  state.settings.recentConfigs = [key, ...(state.settings.recentConfigs || []).filter((k) => k !== key)].slice(0, 12);
  saveSettings();
  renderPin();
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
$("cfgOpenDir").onclick = () => call("open_folder", { which: state.cfg.openRoot + "|" + state.cfg.open }).catch(() => {});
async function saveConfig() {
  if (!isDirty()) return;
  try {
    state.cfg.stamp = await invoke("config_save", { root: state.cfg.openRoot, rel: state.cfg.open, text: $("cfgText").value, stamp: state.cfg.stamp });
    state.cfg.original = $("cfgText").value;
    updateCfgButtons();
    validateCfg();
    toast("Saved, backup kept");
  } catch (e) { cfgStatus("bad", String(e)); toast("Not saved", true); }
}
$("cfgSave").onclick = saveConfig;

// ============================ setup check ============================

let checkResolve = null;

async function runChecks(forLaunch) {
  $("checks").hidden = false;
  $("checksTitle").textContent = "Checking setup…";
  $("checkList").innerHTML = `<div class="empty">Looking at installs, ports and profiles…</div>`;
  $("checksLaunch").hidden = true;
  const profile = state.mode === "host" ? null : state.profileId;
  let list = [];
  try { list = await invoke("health_check", { mode: state.mode, profile }); }
  catch (e) { list = [{ level: "bad", title: String(e) }]; }
  const bad = list.filter((c) => c.level === "bad").length;
  const warn = list.filter((c) => c.level === "warn").length;
  $("checksTitle").textContent = bad ? `${plural(bad, "problem")} found` : warn ? `Ready, ${plural(warn, "warning")}` : "All good";
  // problems first, the passing checks are just reassurance
  const order = { bad: 0, warn: 1, ok: 2 };
  list.sort((a, b) => order[a.level] - order[b.level]);
  $("checkList").innerHTML = list.map((c, i) => `<div class="check-row ${c.level}"><span class="dot"></span>
      <div><b>${esc(c.title)}</b>${c.fix ? `<small>${esc(c.fix)}</small>` : ""}</div>
      ${c.go ? `<button class="ghost sm" data-check-go="${c.go}">Open</button>` : "<span></span>"}</div>`).join("");
  $$("[data-check-go]").forEach((b) => (b.onclick = () => { closeChecks(false); goto(b.dataset.checkGo); }));
  $("checksLaunch").hidden = !forLaunch;
  return { bad, warn };
}

function closeChecks(launch) {
  $("checks").hidden = true;
  if (checkResolve) { checkResolve(launch); checkResolve = null; }
}
$("btnCheck").onclick = () => runChecks(false);
$("checksClose").onclick = () => closeChecks(false);
$("checksLaunch").onclick = () => closeChecks(true);
$("checksRerun").onclick = () => runChecks(!$("checksLaunch").hidden);

/// Resolves true when launch should go ahead. Only stops for real problems.
async function preflight() {
  const { bad } = await runChecks(true);
  if (!bad) { $("checks").hidden = true; return true; }
  return new Promise((r) => (checkResolve = r));
}

// ============================ headless power ============================

$("btnHlPower").onclick = async () => {
  const s = state.status;
  if (s?.headless.owned) {
    const n = hlRaids();
    if (n && !(await confirmBox("Stop headless?", `${plural(n, "raid")} running on the headless will end for everyone in them.`, "Stop headless"))) return;
    await call("stop_headless").catch(() => {});
    toast("Headless stopped, it stays down until you start it");
  } else {
    await call("start_headless").catch(() => {});
    toast("Starting headless");
  }
  pollStatus();
};

function paintHeadlessPower(s) {
  const b = $("btnHlPower");
  b.textContent = s.headless.owned ? "Stop" : "Start";
  b.disabled = !(s.headless.owned || s.canStartHeadless);
  if (state.hlCrashed && s.headless.running) {
    state.hlCrashed = false;
    alertUser("crash", "Headless is back up", "It restarted after the crash.");
  }
}

// ============================ history ============================

const EVENT_TEXT = {
  headless_crash: "headless crash", headless_restart: "headless restart", headless_stop: "headless stopped",
  server_crash: "server crash", server_restart: "server restart", game_exit: "game closed",
};

function when(t) {
  const d = new Date(t * 1000);
  return d.toLocaleDateString(undefined, { day: "numeric", month: "short" }) + " " + d.toTimeString().slice(0, 5);
}

function sessionsFrom(events) {
  const out = [];
  let cur = null;
  for (const e of events) {
    if (e.kind === "launch") {
      if (cur && !cur.end) cur.end = e.t, cur.why = "no clean stop recorded";
      cur = { start: e.t, launch: e, counts: {}, end: null, why: null };
      out.push(cur);
    } else if (e.kind === "stop") {
      if (cur && !cur.end) { cur.end = e.t; cur.why = e.why; }
    } else if (cur && !cur.end && EVENT_TEXT[e.kind]) {
      cur.counts[e.kind] = (cur.counts[e.kind] || 0) + 1;
    }
  }
  return out.reverse();
}

async function loadHistory() {
  const events = await invoke("history", { limit: 3000 }).catch(() => []);
  const sessions = sessionsFrom(events).slice(0, 60);
  const live = state.status?.active;
  $("sessionList").innerHTML = sessions.length ? sessions.map((s, i) => {
    const l = s.launch;
    const open = !s.end && i === 0 && live;
    const len = (s.end || (open ? Date.now() / 1000 : s.start)) - s.start;
    const vers = [l.spt && "SPT " + l.spt, l.fika && "Fika " + l.fika].filter(Boolean).join(" · ");
    const tags = Object.entries(s.counts).map(([k, n]) => {
      const cls = k.endsWith("crash") ? "bad" : "warn";
      return `<span class="mod-tag ${cls}">${EVENT_TEXT[k]}${n > 1 ? " ×" + n : ""}</span>`;
    });
    if (open) tags.unshift(`<span class="mod-tag ok">Running</span>`);
    return `<div class="session"><div class="session-top"><b>${when(s.start)}</b><span class="muted">${MODE_TEXT[l.mode] || l.mode || ""}</span>
        <div class="spacer"></div><span class="muted small">${dur(Math.round(len))}</span></div>
      <div class="session-meta">${esc([l.profile, vers].filter(Boolean).join(" · "))}${s.why ? " · ended: " + esc(s.why) : ""}</div>
      ${tags.length ? `<div class="session-tags">${tags.join("")}</div>` : ""}</div>`;
  }).join("") : `<div class="empty">No sessions yet. They're recorded from the next launch on.</div>`;
  loadBackups();
}

const BK_REASON = { launch: "Before launch", manual: "Manual", "before restore": "Before a restore", spt: "SPT" };
state.bk = { list: [], sel: null, picked: new Set(), src: "all" };

async function loadBackups() {
  state.bk.list = await invoke("backup_list").catch(() => []);
  // keep the selection across refreshes if that backup still exists
  if (state.bk.sel && !state.bk.list.some((b) => bkKey(b) === state.bk.sel)) state.bk.sel = null;
  renderBackups();
}
const bkKey = (b) => b.source + ":" + b.id;

function renderBackups() {
  const { list, sel, src } = state.bk;
  const shown = list.filter((b) => src === "all" || b.source === src);
  let day = "";
  const rows = [];
  for (const b of shown) {
    const d = new Date(b.created * 1000);
    const dayText = d.toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short" });
    if (dayText !== day) { day = dayText; rows.push(`<div class="bk-day">${esc(dayText)}</div>`); }
    const on = bkKey(b) === sel;
    const players = b.profiles.filter((p) => !p.headless);
    const who = players.map((p) => `${esc(p.nickname)} <span>Lv ${p.level}</span>`).join(", ") || "Headless only";
    rows.push(`<div class="bk-row ${on ? "sel" : ""}" data-bk="${esc(bkKey(b))}">
      <span class="bk-radio"></span>
      <div class="bk-time"><b>${d.toTimeString().slice(0, 5)}</b><small>${ago(b.created)}</small></div>
      <div class="bk-who">${who}</div>
      <span class="bk-src ${b.source}">${esc(BK_REASON[b.reason] || b.reason)}</span>
      ${b.source === "app" ? `<button class="textbtn small" data-bk-del="${esc(b.id)}" title="Delete this backup">Delete</button>` : `<span class="bk-gap"></span>`}
    </div>`);
    if (on) {
      rows.push(`<div class="bk-detail">${b.profiles.map((p) => `<label class="bk-prof">
          <input type="checkbox" data-prof="${esc(p.id)}" ${state.bk.picked.has(p.id) ? "checked" : ""}>
          <span class="bk-check"></span><b>${esc(p.nickname || p.id)}</b>
          <small>Lv ${p.level} · ${bytes(p.size)}${p.headless ? " · headless" : ""}</small></label>`).join("")}</div>`);
    }
  }
  $("backupList").innerHTML = rows.length ? rows.join("")
    : `<div class="empty">${list.length ? "None from that source." : "No backups yet. SPT makes its own as you play, and Fika Hub makes one at each launch."}</div>`;

  $$(".bk-row").forEach((el) => (el.onclick = (e) => {
    if (e.target.closest("[data-bk-del]")) return;
    const key = el.dataset.bk;
    if (state.bk.sel === key) return;
    state.bk.sel = key;
    const b = list.find((x) => bkKey(x) === key);
    state.bk.picked = new Set(b.profiles.map((p) => p.id));
    renderBackups();
  }));
  $$("[data-prof]").forEach((c) => (c.onchange = () => {
    c.checked ? state.bk.picked.add(c.dataset.prof) : state.bk.picked.delete(c.dataset.prof);
    paintBkBar();
  }));
  $$("[data-bk-del]").forEach((btn) => (btn.onclick = async () => {
    if (!(await confirmBox("Delete this backup?", "Its copied profiles are removed from disk for good.", "Delete"))) return;
    await call("backup_delete", { id: btn.dataset.bkDel }).catch(() => {});
    loadBackups();
  }));
  paintBkBar();
}

function paintBkBar() {
  const b = state.bk.list.find((x) => bkKey(x) === state.bk.sel);
  const n = b ? b.profiles.filter((p) => state.bk.picked.has(p.id)).length : 0;
  const serverUp = !!state.status?.portOpen;
  $("bkBar").classList.toggle("ready", !!b);
  $("bkSel").innerHTML = !b ? "Pick a backup to load"
    : serverUp ? `<b>${when(b.created)}</b> · stop the server before loading`
    : `<b>${when(b.created)}</b> · ${plural(n, "profile")} selected`;
  $("bkLoad").disabled = !b || !n || serverUp;
}

$$("#bkFilter button").forEach((btn) => (btn.onclick = () => {
  state.bk.src = btn.dataset.src;
  $$("#bkFilter button").forEach((x) => x.classList.toggle("on", x === btn));
  renderBackups();
}));

$("bkLoad").onclick = async () => {
  const b = state.bk.list.find((x) => bkKey(x) === state.bk.sel);
  if (!b) return;
  const picked = b.profiles.filter((p) => state.bk.picked.has(p.id));
  const names = picked.map((p) => p.nickname || p.id).join(", ");
  const from = `${when(b.created)} (${b.source === "spt" ? "SPT backup" : "Fika Hub backup"})`;
  const body = `This copies ${names} from ${from} into user\\profiles and overwrites the current ${picked.length === 1 ? "file" : "files"}.\n\nAnything played since then is lost. A copy of the current profiles is saved first, so you can load that back if this was a mistake.`;
  if (!(await confirmBox("Load this backup?", body, "Overwrite profiles"))) return;
  try {
    const n = await invoke("backup_restore", { source: b.source, id: b.id, profiles: picked.map((p) => p.id) });
    toast(`Loaded ${plural(n, "profile")} from ${when(b.created)}`);
  } catch (e) { toast(String(e), true); }
  state.bk.sel = null;
  loadBackups();
  refreshProfiles();
};

$("histRefresh").onclick = loadHistory;
$("bkNow").onclick = async () => {
  try { const b = await invoke("backup_create"); toast(b ? "Profiles backed up" : "Nothing changed since the last backup"); }
  catch (e) { toast(String(e), true); }
  loadBackups();
};
$("bkOpen").onclick = () => call("backup_open").catch(() => {});
$("diagExport").onclick = async () => {
  $("diagExport").disabled = true;
  try { if (await invoke("export_diagnostics")) toast("Diagnostics saved"); }
  catch (e) { toast(String(e), true); }
  $("diagExport").disabled = false;
};

// ============================ diff ============================

/// Line diff via LCS on the middle part, after trimming the shared head and tail.
function diffLines(a, b) {
  const A = a.split("\n"), B = b.split("\n");
  let s = 0;
  while (s < A.length && s < B.length && A[s] === B[s]) s++;
  let ea = A.length, eb = B.length;
  while (ea > s && eb > s && A[ea - 1] === B[eb - 1]) { ea--; eb--; }
  const out = A.slice(0, s).map((t) => ["=", t]);
  const n = ea - s, m = eb - s;
  if (n * m > 4e6) {
    // too big to align, a plain before/after is still readable
    A.slice(s, ea).forEach((t) => out.push(["-", t]));
    B.slice(s, eb).forEach((t) => out.push(["+", t]));
  } else {
    const L = new Uint32Array((n + 1) * (m + 1));
    for (let i = n - 1; i >= 0; i--)
      for (let j = m - 1; j >= 0; j--)
        L[i * (m + 1) + j] = A[s + i] === B[s + j] ? L[(i + 1) * (m + 1) + j + 1] + 1 : Math.max(L[(i + 1) * (m + 1) + j], L[i * (m + 1) + j + 1]);
    let i = 0, j = 0;
    while (i < n || j < m) {
      if (i < n && j < m && A[s + i] === B[s + j]) { out.push(["=", A[s + i]]); i++; j++; }
      else if (j < m && (i === n || L[i * (m + 1) + j + 1] > L[(i + 1) * (m + 1) + j])) { out.push(["+", B[s + j]]); j++; }
      else { out.push(["-", A[s + i]]); i++; }
    }
  }
  A.slice(ea).forEach((t) => out.push(["=", t]));
  return out;
}

function diffHtml(oldText, newText) {
  if (oldText === newText) return `<div class="empty">No differences.</div>`;
  const ops = diffLines(oldText, newText);
  const html = [];
  const CTX = 3;
  for (let k = 0; k < ops.length; k++) {
    const [op, t] = ops[k];
    if (op !== "=") { html.push(`<div class="${op === "+" ? "add" : "del"}">${op} ${esc(t)}</div>`); continue; }
    // collapse long unchanged stretches, keep a few lines around each change
    let e = k;
    while (e < ops.length && ops[e][0] === "=") e++;
    const runLen = e - k;
    const head = k === 0 ? 0 : CTX, tail = e === ops.length ? 0 : CTX;
    if (runLen > head + tail + 2) {
      for (let x = k; x < k + head; x++) html.push(`<div class="same">  ${esc(ops[x][1])}</div>`);
      html.push(`<div class="skip">${runLen - head - tail} unchanged lines</div>`);
      for (let x = e - tail; x < e; x++) html.push(`<div class="same">  ${esc(ops[x][1])}</div>`);
    } else {
      for (let x = k; x < e; x++) html.push(`<div class="same">  ${esc(ops[x][1])}</div>`);
    }
    k = e - 1;
  }
  return `<div class="diff">${html.join("")}</div>`;
}

// ============================ snapshots + file history ============================

function openDrawer(title, html) {
  $("drawerTitle").textContent = title;
  $("drawerBody").innerHTML = html;
  $("drawer").hidden = false;
}
$("drawerClose").onclick = () => ($("drawer").hidden = true);

const CHANGE_TEXT = { changed: "Changed", missing: "Deleted since", added: "New since" };

async function openSnapshots(selectId) {
  const root = state.cfg.root;
  if (!root) return;
  const label = $$("#cfgRoots button").find((b) => b.dataset.root === root)?.textContent || root;
  const snaps = await invoke("snapshot_list", { root }).catch(() => []);
  openDrawer(`Snapshots · ${label} configs`, `<div class="drawer-side">
      <div class="snap-new"><input class="text-in" id="snapName" placeholder="Name, e.g. Before SVM tweak"><button class="primary sm" id="snapTake">Take</button></div>
      <div id="snapList">${snaps.length ? snaps.map((s) => `<div class="pick" data-snap="${s.id}"><b>${esc(s.name)}</b><small>${when(s.created)} · ${s.files.length} files</small></div>`).join("")
        : `<div class="empty">No snapshots yet. Take one before changing a lot of settings.</div>`}</div>
    </div><div class="drawer-main" id="snapMain"><div class="empty">Pick a snapshot to compare it with the files as they are now.</div></div>`);
  $("snapTake").onclick = async () => {
    $("snapTake").disabled = true;
    try { const s = await invoke("snapshot_create", { root, name: $("snapName").value }); toast(`Saved ${s.files.length} files`); openSnapshots(s.id); }
    catch (e) { toast(String(e), true); $("snapTake").disabled = false; }
  };
  $$("[data-snap]").forEach((el) => (el.onclick = () => showSnapshot(root, snaps.find((s) => s.id === el.dataset.snap))));
  const pre = snaps.find((s) => s.id === selectId);
  if (pre) showSnapshot(root, pre);
}

async function showSnapshot(root, snap) {
  $$("[data-snap]").forEach((el) => el.classList.toggle("sel", el.dataset.snap === snap.id));
  const main = $("snapMain");
  main.innerHTML = `<div class="empty">Comparing…</div>`;
  let changes;
  try { changes = await invoke("snapshot_compare", { root, id: snap.id }); }
  catch (e) { main.innerHTML = `<div class="empty">${esc(e)}</div>`; return; }
  const restorable = changes.filter((c) => c.status !== "added");
  main.innerHTML = `<div class="drawer-bar"><b>${esc(snap.name)}</b><span class="muted small">${when(snap.created)}</span><div class="spacer"></div>
      <button class="ghost sm" id="snapDel">Delete</button>
      <button class="primary sm" id="snapRestoreAll" ${restorable.length ? "" : "disabled"}>Restore ${restorable.length ? plural(restorable.length, "file") : "nothing"}</button></div>
    ${changes.length ? changes.map((c) => `<div class="change" data-rel="${esc(c.rel)}" data-status="${c.status}"><span>${esc(c.rel)}</span><i class="mod-tag ${c.status === "changed" ? "warn" : c.status === "missing" ? "bad" : ""}">${CHANGE_TEXT[c.status]}</i></div>`).join("")
      : `<div class="empty">Everything matches this snapshot.</div>`}
    <div id="snapDiff"></div>`;
  $("snapDel").onclick = async () => {
    if (!(await confirmBox("Delete snapshot?", `${snap.name} is removed for good. Your current configs aren't touched.`, "Delete"))) return;
    await call("snapshot_delete", { root, id: snap.id }).catch(() => {});
    openSnapshots();
  };
  $("snapRestoreAll").onclick = async () => {
    if (!(await confirmBox("Restore snapshot?", `${plural(restorable.length, "file")} go back to how they were in "${snap.name}". Each current file is backed up first. Files added since stay as they are.`, "Restore"))) return;
    await restoreSnap(root, snap, null);
  };
  $$(".change").forEach((el) => (el.onclick = async () => {
    $$(".change").forEach((x) => x.classList.toggle("sel", x === el));
    const rel = el.dataset.rel, st = el.dataset.status;
    const old = st === "added" ? "" : await invoke("snapshot_read", { root, id: snap.id, rel }).catch(() => "");
    const now = st === "missing" ? "" : (await invoke("config_read", { root, rel }).catch(() => ({ text: "" }))).text;
    $("snapDiff").innerHTML = `<div class="drawer-bar"><b>${esc(rel)}</b><span class="muted small">red is the snapshot, green is now</span><div class="spacer"></div>
        ${st !== "added" ? `<button class="ghost sm" id="snapRestoreOne">Restore this file</button>` : ""}</div>${diffHtml(old, now)}`;
    const one = $("snapRestoreOne");
    if (one) one.onclick = () => restoreSnap(root, snap, [rel]);
  }));
}

async function restoreSnap(root, snap, only) {
  if (isDirty() && only?.includes(state.cfg.open) && !(await leaveDirty())) return;
  try {
    const n = await invoke("snapshot_restore", { root, id: snap.id, only });
    toast(n ? `Restored ${plural(n, "file")}` : "Already matched");
    if (state.status?.server.running && root === "server") toast("Restart the server for server configs to apply");
  } catch (e) { toast(String(e), true); }
  if (state.cfg.open && state.cfg.openRoot === root) openConfig(state.cfg.open);
  showSnapshot(root, snap);
}
$("cfgSnapBtn").onclick = () => openSnapshots();

async function openFileHistory() {
  const root = state.cfg.openRoot, rel = state.cfg.open;
  if (!rel) return;
  const [baks, snaps] = await Promise.all([
    invoke("config_backups", { root, rel }).catch(() => []),
    invoke("snapshot_list", { root }).catch(() => []),
  ]);
  const inSnaps = snaps.filter((s) => s.files.includes(rel));
  const items = [
    ...baks.map((b) => ({ key: "b:" + b.name, title: when(b.saved), sub: `Before a save · ${bytes(b.size)}`, load: () => invoke("config_backup_read", { root, rel, name: b.name }) })),
    ...inSnaps.map((s) => ({ key: "s:" + s.id, title: s.name, sub: `Snapshot · ${when(s.created)}`, load: () => invoke("snapshot_read", { root, id: s.id, rel }) })),
  ];
  openDrawer(`History · ${rel.split("\\").pop()}`, `<div class="drawer-side">${items.length
      ? items.map((it) => `<div class="pick" data-hist="${esc(it.key)}"><b>${esc(it.title)}</b><small>${esc(it.sub)}</small></div>`).join("")
      : `<div class="empty">No earlier versions. One is kept each time you save here.</div>`}</div>
    <div class="drawer-main" id="histMain"><div class="empty">Pick a version to compare it with what's in the editor.</div></div>`);
  $$("[data-hist]").forEach((el) => (el.onclick = async () => {
    $$("[data-hist]").forEach((x) => x.classList.toggle("sel", x === el));
    const it = items.find((x) => x.key === el.dataset.hist);
    let text;
    try { text = await it.load(); } catch (e) { $("histMain").innerHTML = `<div class="empty">${esc(e)}</div>`; return; }
    $("histMain").innerHTML = `<div class="drawer-bar"><b>${esc(it.title)}</b><span class="muted small">red is this version, green is the editor</span><div class="spacer"></div>
        <button class="primary sm" id="histLoad">Load into editor</button></div>${diffHtml(text, $("cfgText").value)}`;
    $("histLoad").onclick = () => {
      // lands as an unsaved edit, so nothing changes on disk until Save
      $("cfgText").value = text;
      $("cfgText").oninput();
      $("drawer").hidden = true;
      toast("Loaded. Save to keep it, Revert to drop it");
    };
  }));
}
$("cfgHistBtn").onclick = openFileHistory;

// ============================ settings values ============================

function applySettingValues() {
  $$("[data-set-val]").forEach((i) => {
    if (document.activeElement !== i) i.value = state.settings[i.dataset.setVal] ?? "";
  });
}
$$("[data-set-val]").forEach((i) => (i.onchange = async () => {
  const k = i.dataset.setVal;
  state.settings[k] = i.type === "number" ? Math.max(+i.min || 0, Math.min(+i.max || 1e9, Math.round(+i.value || 0))) : i.value.trim();
  i.value = state.settings[k];
  await saveSettings();
  loadNextRestart();
}));

async function loadNextRestart() {
  const t = await invoke("next_restart").catch(() => 0);
  $("nextRestartText").textContent = t ? `Next restart ${when(t)}.` : "Local time on this PC.";
}

$("discordTest").onclick = async () => {
  $("discordTest").disabled = true;
  try { await invoke("discord_test"); toast("Posted to Discord"); } catch (e) { toast(String(e), true); }
  $("discordTest").disabled = false;
};


boot();
