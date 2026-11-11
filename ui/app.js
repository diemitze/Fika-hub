"use strict";

const T = window.__TAURI__;
const invoke = T.core.invoke;
const appWindow = T.window.getCurrentWindow();
const $ = (id) => document.getElementById(id);
const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];

const MAPS = {
  bigmap: "Customs",
  factory4_day: "Factory",
  factory4_night: "Night Factory",
  interchange: "Interchange",
  laboratory: "The Lab",
  lighthouse: "Lighthouse",
  rezervbase: "Reserve",
  shoreline: "Shoreline",
  woods: "Woods",
  tarkovstreets: "Streets",
  sandbox: "Ground Zero",
  sandbox_high: "Ground Zero 21+",
  labyrinth: "Labyrinth",
};
const ACTIVITY = ["In menu", "In raid", "In stash", "In hideout", "On the flea"];
const MODE_TEXT = { full: "Full stack", host: "Host only", join: "Game only", headless: "Headless only" };
// modes that never start the player's game
const noGame = (m) => m === "host" || m === "headless";
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
  logBad: [0, 0, 0],
  badSeen: [0, 0, 0],
  bootPhase: null,
  runs: [[], [], []],
  runView: ["current", "current", "current"],
  stream: 0,
  level: "all",
  cfg: { root: null, entries: [], open: null, stamp: null, original: "" },
};

// ============================ helpers ============================

const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
const mapName = (id) => MAPS[(id || "").toLowerCase()] || id || "Unknown";
const initials = (n) =>
  (n || "?")
    .replace(/[^A-Za-z0-9]/g, "")
    .slice(0, 2)
    .toUpperCase() || "?";

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
  while (b >= 1024 && i < u.length - 1) {
    b /= 1024;
    i++;
  }
  return (b >= 100 || i === 0 ? b.toFixed(0) : b.toFixed(1)) + " " + u[i];
}
function dur(s) {
  if (!s && s !== 0) return "-";
  const h = Math.floor(s / 3600),
    m = Math.floor((s % 3600) / 60);
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
    const done = (v) => {
      $("modal").hidden = true;
      $("modalYes").onclick = $("modalNo").onclick = null;
      resolve(v);
    };
    $("modalYes").onclick = () => done(true);
    $("modalNo").onclick = () => done(false);
  });
}

async function call(cmd, args) {
  try {
    return await invoke(cmd, args);
  } catch (e) {
    toast(String(e), true);
    throw e;
  }
}

// ============================ window + nav ============================

$("winMin").onclick = () => appWindow?.minimize();
$("winMax").onclick = () => appWindow?.toggleMaximize();
$("winClose").onclick = async () => {
  const s = state.status;
  const owns = s && (s.server.owned || s.headless.owned || s.game.owned);
  if (
    owns &&
    !(await confirmBox("Quit Fika Hub?", "This stops the server, headless and game that were started from here.", "Stop and quit"))
  )
    return;
  invoke("quit");
};

function goto(page) {
  state.page = page;
  $$(".rail-btn").forEach((b) => b.classList.toggle("active", b.dataset.page === page));
  moveRailInd();
  $$(".page").forEach((p) => p.classList.toggle("active", p.id === "page-" + page));
  if (page === "console") {
    state.logSeen = [...state.logErr];
    state.badSeen[state.stream] = state.logBad[state.stream];
    renderRuns();
    renderLog(true);
    updateBadges();
  }
  if (page === "configs" && !state.cfg.root) loadConfigRoots();
  if (page === "settings") {
    loadConnection();
    loadNextRestart();
  }
  if (page === "history") loadHistory();
  saveUi();
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
$$("[data-goto-log]").forEach(
  (b) =>
    (b.onclick = () => {
      setStream(+b.dataset.gotoLog);
      goto("console");
    }),
);

document.addEventListener("keydown", (e) => {
  if (e.ctrlKey && e.key.toLowerCase() === "s" && state.page === "configs") {
    e.preventDefault();
    saveConfig();
  }
  if (e.ctrlKey && e.key.toLowerCase() === "f" && state.page === "console") {
    e.preventDefault();
    $("logSearch").focus();
  }
  if (e.ctrlKey && e.key.toLowerCase() === "f" && state.page === "configs") {
    e.preventDefault();
    $(state.cfgView === "raw" ? "cfgFind" : "cfgKeySearch").focus();
  }
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
  await restoreUi();
  if (b.hostProblem) showSetup();
  await refreshProfiles();
  loadConnection();
  pollStatus();
  setInterval(pollStatus, 1000);
  setInterval(pollLogs, 500);
  setInterval(pollLobby, 3000);
  setInterval(refreshProfiles, 15000);
}

// ---- remembered UI state (ui.json next to settings) ----
const UI_TOGGLES = ["logWrap", "logQuiet", "logFollow", "cfgAll"];
let uiReady = false,
  uiT;
function saveUi() {
  if (!uiReady) return;
  clearTimeout(uiT);
  uiT = setTimeout(() => {
    const ui = {
      page: state.page,
      stream: state.stream,
      level: state.level,
      cfgRoot: state.cfg.root,
      bkSrc: state.bk.src,
      cfgView: state.cfgView,
    };
    UI_TOGGLES.forEach((id) => (ui[id] = $(id).checked));
    invoke("ui_state_set", { ui }).catch(() => {});
  }, 400);
}
UI_TOGGLES.forEach((id) => $(id).addEventListener("change", saveUi));

async function restoreUi() {
  const ui = (await invoke("ui_state_get").catch(() => null)) || {};
  UI_TOGGLES.forEach((id) => typeof ui[id] === "boolean" && ($(id).checked = ui[id]));
  $("logView").classList.toggle("wrap", $("logWrap").checked);
  if (["all", "warn", "error", "context"].includes(ui.level)) {
    state.level = ui.level;
    $$("#levelSeg button").forEach((x) => x.classList.toggle("on", x.dataset.level === ui.level));
  }
  if ([0, 1, 2].includes(ui.stream)) {
    state.stream = ui.stream;
    $$(".tab").forEach((t) => t.classList.toggle("active", +t.dataset.stream === ui.stream));
  }
  if (typeof ui.cfgRoot === "string") state.cfg.root = ui.cfgRoot;
  if (ui.cfgView === "raw") setCfgView("raw");
  if (["all", "spt", "app"].includes(ui.bkSrc)) {
    state.bk.src = ui.bkSrc;
    $$("#bkFilter button").forEach((x) => x.classList.toggle("on", x.dataset.src === ui.bkSrc));
  }
  if (ui.page && ui.page !== "dashboard" && $("page-" + ui.page)) goto(ui.page);
  uiReady = true;
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
  $("setHeadless").textContent = b.headlessDir || b.settings.headlessDir || "Not found";
  $("setHeadlessNote").textContent =
    b.headlessProblem || (b.headlessSeparate ? "Separate from the host install." : "Same as the host install.");
  $("setHeadlessNote").className = b.headlessProblem ? "bad" : "";
  $("resetHeadless").hidden = !b.headlessSeparate;

  $$("[data-set]").forEach((i) => (i.checked = !!b.settings[i.dataset.set]));
  applySettingValues();
  $$("#displaySeg button").forEach((x) => x.classList.toggle("on", x.dataset.display === (b.settings.displayMode || "")));
  $("aboutText").textContent = `Fika Hub ${b.appVersion} · by 20fpsguy`;
}

function showSetup() {
  const b = state.boot;
  $("setup").hidden = false;
  $("stepHost").classList.toggle("done", !b.hostProblem);
  $("setupHostText").textContent = b.hostDir || "Where the SPT server lives.";
  $("setupHostBtn").hidden = b.hostFromExe;
  $("stepPlayer").classList.toggle("done", !!b.playerDir && !b.playerProblem);
  $("setupPlayerText").textContent = b.playerDir || "The copy you play on. Skip it if you only host.";
  $("stepHeadless").classList.toggle("done", !b.headlessProblem);
  $("setupHeadlessText").textContent = b.headlessDir || b.headlessProblem || "Only if the headless isn't in the host install.";
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
$("setupHostBtn").onclick = async () => {
  await browse("host");
  showSetup();
};
$("setupPlayerBtn").onclick = async () => {
  await browse("player");
  showSetup();
};
$("setupDone").onclick = () => ($("setup").hidden = true);
$("browseHost").onclick = () => browse("host");
$("browsePlayer").onclick = () => browse("player");
$("setupHeadlessBtn").onclick = async () => {
  await browse("headless");
  showSetup();
};
$("browseHeadless").onclick = () => browse("headless");
$("resetHeadless").onclick = async () => {
  try {
    applyBoot(await invoke("set_dir", { kind: "headless", path: "" }));
  } catch (e) {
    toast(String(e), true);
  }
};

// ============================ settings ============================

function saveSettings() {
  return invoke("save_settings", { settings: state.settings });
}
$$("[data-set]").forEach(
  (i) =>
    (i.onchange = () => {
      state.settings[i.dataset.set] = i.checked;
      saveSettings();
    }),
);
$$("#displaySeg button").forEach(
  (b) =>
    (b.onclick = () => {
      state.settings.displayMode = b.dataset.display;
      $$("#displaySeg button").forEach((x) => x.classList.toggle("on", x === b));
      saveSettings();
      toast(b.dataset.display ? `Game will start ${b.textContent.toLowerCase()}` : "Window mode left alone");
    }),
);

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
  if (document.activeElement !== $("headlessProfile")) $("headlessProfile").value = state.settings.headlessProfile || "";
  $("serverAddrNote").textContent = state.settings.serverAddress
    ? `Game only and Headless only connect to ${c.gameAddr}.`
    : "Game only and Headless only connect to this PC.";
}

async function copyLan() {
  const c = state.conn;
  if (!c?.lanIp) return;
  try {
    await navigator.clipboard.writeText(`${c.lanIp}:${c.port}`);
    toast("Address copied");
  } catch {
    toast("Clipboard unavailable", true);
  }
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
  if (LOOPBACK(c.bindIp))
    rows.push(
      checkRow(
        "bad",
        `SPT only listens on this PC (ip is ${c.bindIp || "unset"} in http.json). Set it to 0.0.0.0 so friends can reach it.`,
      ),
    );
  else rows.push(checkRow("ok", `SPT listens on ${c.bindIp}.`));
  if (LOOPBACK(c.backendIp))
    rows.push(
      checkRow(
        "warn",
        `backendIp is ${c.backendIp || "unset"}, which sends friends back to their own PC. Set it to ${c.lanIp || "your LAN IP"}, or your public IP for internet play.`,
      ),
    );
  else rows.push(checkRow("ok", `backendIp is ${c.backendIp}.`));
  if (!c.lanIp) rows.push(checkRow("bad", "No LAN address found. Is this PC on a network?"));
  else if (!state.status?.portOpen) rows.push(checkRow("warn", "Start the server to test the port."));
  else {
    $("connCheck").disabled = true;
    const ok = await invoke("check_reach", { addr: `${c.lanIp}:${c.port}` }).catch(() => false);
    $("connCheck").disabled = false;
    rows.push(
      ok
        ? checkRow("ok", `Port ${c.port} answers on ${c.lanIp}. Windows Firewall can still block other PCs, this test runs from here.`)
        : checkRow("bad", `Port ${c.port} doesn't answer on ${c.lanIp}. Check the ip above, then restart the server.`),
    );
  }
  $("connChecks").innerHTML = rows.join("");
};

$("serverAddr").onchange = async () => {
  state.settings.serverAddress = $("serverAddr").value.trim();
  await saveSettings();
  loadConnection();
};
$("headlessProfile").onchange = async () => {
  state.settings.headlessProfile = $("headlessProfile").value.trim();
  await saveSettings();
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
  document.querySelector(".cmd-who").style.opacity = noGame(state.mode) ? ".45" : "";
}
$$("#modeSeg button").forEach(
  (b) =>
    (b.onclick = () => {
      if (state.status?.active) return toast("Stop first to change mode");
      state.mode = b.dataset.mode;
      state.settings.launchMode = state.mode;
      saveSettings();
      renderMode();
    }),
);

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
    ? players
        .map(
          (x) => `<div class="who-item ${x.id === state.profileId ? "sel" : ""}" data-id="${esc(x.id)}">
        <div class="who-avatar">${esc(initials(x.nickname))}</div>
        <div><b>${esc(x.nickname)}</b><small>Lv ${x.level} · ${esc(x.username)}${x.valid ? "" : " · incomplete"}</small></div></div>`,
        )
        .join("")
    : `<div class="who-empty">No player profiles on the host yet.</div>`;
  $$(".who-item").forEach(
    (el) =>
      (el.onclick = () => {
        state.profileId = el.dataset.id;
        state.settings.lastProfile = state.profileId;
        saveSettings();
        $("whoMenu").hidden = true;
        renderWho();
        renderOperator();
      }),
  );
}
$("whoBtn").onclick = (e) => {
  e.stopPropagation();
  $("whoMenu").hidden = !$("whoMenu").hidden;
};
document.addEventListener("click", (e) => {
  if (!e.target.closest(".cmd-who")) $("whoMenu").hidden = true;
});

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
  $("opRaids").textContent = `${p.survived} of ${p.raids} raids`;
  $("opLast").textContent = "Last raid " + ago(p.lastSession);
  $("opKd").textContent = kd;
  $("opKills").textContent = `${human(p.kills)} kills, ${human(p.deaths)} deaths`;
  $("opPmc").textContent = human(p.pmcKills);
  $("opBoss").textContent = human(p.bossKills);
  $("opHs").textContent = human(p.headshots) + (p.kills ? ` · ${Math.round((p.headshots / p.kills) * 100)}%` : "");
  $("opStreak").textContent = p.longestStreak + (p.winStreak ? ` (now ${p.winStreak})` : "");
  $("opRub").textContent = "₽ " + human(p.roubles);
  $("opTime").textContent = p.lifetimeSecs ? dur(p.lifetimeSecs).split(" ").slice(0, 2).join(" ") : "-";
  $("opSaved").textContent = "saved " + dur(p.savedAgo).split(" ")[0] + " ago";
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
  if (!noGame(state.mode) && !p) return toast("Pick who you're playing as", true);
  if (state.settings.checkBeforeLaunch) {
    if (!(await preflight())) return;
  } else if (!noGame(state.mode)) {
    if (
      !p.valid &&
      !(await confirmBox(
        "Profile looks incomplete",
        `${p.nickname} is missing parts of its save. The game may refuse it. Launch anyway?`,
        "Launch anyway",
      ))
    )
      return;
  }
  if (!state.settings.checkBeforeLaunch && state.mismatch?.length && state.settings.warnMismatch && !noGame(state.mode)) {
    if (
      !(await confirmBox(
        "Versions don't match",
        state.mismatch.join("\n") + "\n\nMismatched installs usually fail to connect.",
        "Launch anyway",
      ))
    )
      return;
  }
  await call("launch", { mode: state.mode, profile: noGame(state.mode) ? null : state.profileId }).catch(() => {});
  pollStatus();
};

const hlRaids = () => (state.lobby?.raids || []).filter((r) => r.headless && r.status !== 2).length;
const plural = (n, w) => `${n} ${w}${n === 1 ? "" : "s"}`;

$("btnRestartHl").onclick = async () => {
  const n = hlRaids();
  if (
    n &&
    !(await confirmBox(
      "Restart headless?",
      `${plural(n, "raid")} running on the headless will end for everyone in them. The server and connected players stay up.`,
      "Restart headless",
    ))
  )
    return;
  await call("restart_headless").catch(() => {});
  toast("Restarting headless");
};
$("btnRestartSrv").onclick = async () => {
  const players = state.lobby?.players?.length || 0;
  const who = players
    ? `${plural(players, "connected player")} (your game included) will be disconnected`
    : "Anyone connected, your game included, will be disconnected";
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
  try {
    s = await invoke("status");
  } catch {
    return;
  }
  const prev = state.status;
  state.status = s;
  // a stack we started lost its server without being told to
  if (prev?.active && s.active && prev.server.owned && prev.server.running && !s.server.running && !state.expectSrvExit) {
    alertUser("crash", "SPT server stopped", "The server started from Fika Hub is no longer running.");
  }
  if (s.server.running || !s.active) state.expectSrvExit = false;

  for (const [key, node] of [
    ["server", "nodeServer"],
    ["headless", "nodeHeadless"],
    ["game", "nodeGame"],
  ]) {
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
  const { booting, stageText, prog, meta } = bootInfo(s);
  go.classList.toggle("running", s.active);
  go.classList.toggle("busy", busy);
  $("goLabel").textContent = s.active ? "STOP" : "LAUNCH";
  $("goSub").textContent = s.active
    ? booting
      ? `${stageText} · ${meta}`
      : busy
        ? s.stage + "…"
        : MODE_TEXT[s.mode] + " running"
    : MODE_TEXT[state.mode];
  $("goFill").style.width = s.active ? prog + "%" : "0";
  $("whoBtn").classList.toggle("locked", s.active);

  // titlebar pill
  const ext = !s.active && (s.server.running || s.headless.running || s.game.running);
  const up = Math.max(s.server.uptime || 0, s.headless.uptime || 0, s.game.uptime || 0);
  if (busy) setPill("busy", stageText, meta, prog);
  else if (s.active) setPill("live", "Live", `${MODE_TEXT[s.mode] || ""} · ${dur(up)}`, null);
  else if (ext) setPill("ext", "Running elsewhere", null, null);
  else setPill("", "Idle", null, null);
  document.body.classList.toggle("is-live", s.active && !busy);
  document.body.classList.toggle("is-busy", busy);

  $$(".tab").forEach((t) => t.classList.toggle("live", s[STREAM_NAMES[+t.dataset.stream]].running));
}

// while the server boots, show what it's doing and how long the last boots took
function bootInfo(s) {
  const booting = s.active && s.progress < 100 && s.bootElapsed != null && !s.portOpen;
  if (!booting) return { booting, stageText: s.stage, prog: s.progress, meta: s.progress + "%" };
  const e = s.bootElapsed,
    x = s.bootExpect;
  return {
    booting,
    stageText: state.bootPhase || "Booting server",
    prog: Math.round(x ? 12 + 50 * Math.min(e / Math.max(x, 1), 0.97) : 12 + 50 * (1 - Math.exp(-e / 45))),
    meta: x ? (x - e > 3 ? `~${fmtLeft(x - e)} left` : "almost ready") : fmtLeft(e),
  };
}

function paintNode(el, p, key, s) {
  const starting =
    s.active &&
    !p.running &&
    s.progress < 100 &&
    ((key === "server" && (s.mode === "full" || s.mode === "host")) ||
      (key === "headless" && s.mode !== "join" && s.progress >= 65) ||
      (key === "game" && !noGame(s.mode) && s.progress >= 85));
  const serverWaiting = key === "server" && p.running && !s.portOpen;
  el.classList.toggle("on", p.running && p.owned && !serverWaiting);
  el.classList.toggle("ext", p.running && !p.owned);
  el.classList.toggle("wait", starting || serverWaiting);
  el.classList.toggle("off", !p.running && !starting);
  el.querySelector(".node-state").textContent = serverWaiting
    ? bootInfo(s).booting
      ? `Booting · ${bootInfo(s).meta}`
      : "Booting…"
    : starting
      ? "Starting…"
      : p.running
        ? p.owned
          ? "Online"
          : "Running elsewhere"
        : "Offline";
  el.querySelector(".v-up").textContent = p.running ? dur(p.uptime) : "-";
  el.querySelector(".v-cpu").textContent = p.running ? Math.round(p.cpu) + "%" : "-";
  el.querySelector(".v-ram").textContent = p.running ? bytes(p.ram) : "-";
}

function drawSpark(svg, raw) {
  const n = 60;
  // pad with the oldest sample so a fresh process draws a flat line, not a stub
  const data = Array(n - raw.length)
    .fill(raw[0] || 0)
    .concat(raw);
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
  try {
    renderLobby(await invoke("lobby", { session: state.profileId }));
  } catch {
    renderLobby(null);
  }
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
    ? raids
        .map((r) => {
          const cls = r.status === 0 ? "loading" : r.status === 2 ? "done" : "";
          const st = ["Loading", "In raid", "Finished"][r.status] || "Unknown";
          return `<div class="raid ${cls}">
          <div class="raid-count">${r.players}<small>${r.players === 1 ? "player" : "players"}</small></div>
          <b>${esc(mapName(r.location))}</b>
          <div class="raid-meta">Started by ${esc(r.host || "?")}</div>
          <div class="raid-tags"><span class="tag ${r.status === 1 ? "live" : ""}">${st}</span><span class="tag">${r.side === 1 ? "Scav" : "PMC"}</span>${r.headless ? '<span class="tag hl">Headless hosted</span>' : ""}</div>
        </div>`;
        })
        .join("")
    : `<div class="empty"><svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></svg>${l?.online ? "No raids running." : "Start the server to see raids."}</div>`;

  $("playerList").innerHTML = players.length
    ? players
        .map((p) => {
          const inRaid = p.activity === 1;
          const where = inRaid && p.location ? " · " + mapName(p.location) : "";
          const since = p.since ? " · " + dur(Math.max(0, Math.floor(Date.now() / 1000 - p.since))).split(" ")[0] : "";
          return `<div class="player"><div class="who-avatar">${esc(initials(p.nickname))}</div>
          <div class="p-name"><b>${esc(p.nickname)}</b><small>Level ${p.level}</small></div>
          <span class="act ${inRaid ? "raid" : ""}">${ACTIVITY[p.activity] || "Unknown"}${esc(where)}${since}</span></div>`;
        })
        .join("")
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
  if (!l?.online) {
    state.seenPlayers = null;
    return;
  }
  const me = state.profiles.find((p) => p.id === state.profileId)?.nickname;
  const now = new Set((l.players || []).map((p) => p.nickname).filter((n) => n && n !== me));
  // the first list after the server comes up is a baseline, not a wave of joins
  if (state.seenPlayers) {
    const joined = [...now].filter((n) => !state.seenPlayers.has(n));
    const left = [...state.seenPlayers].filter((n) => !now.has(n));
    if (joined.length)
      alertUser("players", joined.length === 1 ? `${joined[0]} connected` : `${joined.length} players connected`, joined.join(", "));
    if (left.length)
      alertUser("players", left.length === 1 ? `${left[0]} disconnected` : `${left.length} players disconnected`, left.join(", "));
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

// Only these light the rail badge: crashes and fatal errors, not the routine mod noise.
const SERIOUS_RE = /^\[Fika Hub\] .*(exited|didn't|couldn't|failed)|unhandled exception|^\[(fatal|critical)\b|\]\[(fatal|critical)\]/i;

// Turns a server output line into a short "what is it doing" label, or null to keep the last one.
function phaseOf(text) {
  let t = text.replace(/^\[\d{4}-[^\]]*\]/, "").replace(/^\[(information|warning|error|debug|trace|critical|fatal)\]/i, "");
  let ns = null;
  const m = t.match(/^\[([\w.]+)\]\s*/);
  if (m && m[1].includes(".")) {
    ns = m[1];
    t = t.slice(m[0].length);
  }
  t = t.trim();
  let x;
  if (!t || /^[│┌└─]/.test(t) || ns?.includes("Watermark")) return null;
  if ((x = t.match(/ModLoader: loading: (\d+)/))) return `Loading ${x[1]} mods`;
  if (/^Mod: /.test(t)) return "Checking mods";
  if (/Importing database/i.test(t)) return "Importing database";
  if (/Database import (finished|took)/i.test(t)) return "Database ready";
  if (/executing startup callbacks/i.test(t)) return "Starting up mods";
  if (ns?.startsWith("SPTarkov.Server.Core.Services.Hosted")) return "Almost ready";
  if ((x = t.match(/^\[([^\]]{2,30})\]/))) return "Loading " + x[1].replace(/^Manimal-/, "");
  if (ns && !ns.startsWith("SPTarkov")) return "Loading " + ns.split(".")[0].replace(/^_/, "");
  return null;
}

function fmtLeft(s) {
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s`;
}

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
    try {
      c = await invoke("logs_since", { stream: i, after: state.logSeq[i] });
    } catch {
      continue;
    }
    if (c.reset) {
      state.logs[i] = [];
      state.logErr[i] = 0;
      state.logSeen[i] = 0;
      state.logBad[i] = state.badSeen[i] = 0;
      resetRuns(i);
      if (i === state.stream && state.page === "console") {
        renderRuns();
        renderLog(true);
      }
    }
    if (!c.lines.length) continue;
    state.logSeq[i] = c.lines[c.lines.length - 1][0];
    let newRun = false;
    const added = c.lines.map(([, text]) => {
      if (text.startsWith(RUN_MARK)) {
        // each run gets its own error count, so a restarted headless starts clean
        state.runs[i].push({ id: curRun(i) + 1, label: text.slice(RUN_MARK.length), time: new Date() });
        state.logErr[i] = state.logSeen[i] = 0;
        state.logBad[i] = state.badSeen[i] = 0;
        if (i === 0) state.bootPhase = null;
        newRun = true;
      }
      const l = { text, lvl: levelOf(text), run: curRun(i) };
      if (l.lvl === "error") state.logErr[i]++;
      if (SERIOUS_RE.test(text)) state.logBad[i]++;
      if (i === 0) state.bootPhase = phaseOf(text) || state.bootPhase;
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
  if (state.page === "console") {
    state.logSeen[state.stream] = state.logErr[state.stream];
    state.badSeen[state.stream] = state.logBad[state.stream];
  }
  updateBadges();
}

function updateBadges() {
  let total = 0;
  $$(".tab").forEach((t) => {
    const i = +t.dataset.stream;
    const badge = t.querySelector(".tab-err");
    const shown = Math.max(state.logErr[i], state.logBad[i]);
    badge.hidden = shown === 0;
    badge.textContent = shown > 999 ? "999+" : shown;
    badge.title = state.logBad[i] ? "Something crashed or failed in this run" : "Errors in the current run, mostly harmless mod noise";
    badge.classList.toggle("bad", state.logBad[i] > 0);
    if (state.page !== "console" || i !== state.stream) total += Math.max(0, state.logBad[i] - state.badSeen[i]);
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
  view.innerHTML = lines.length ? lines.map(lineHtml).join("") : `<div class="log-empty">${total ? "No lines match." : emptyText()}</div>`;
  $("logStats").textContent = `${lines.filter((l) => l.lvl !== "gap").length} of ${total} lines`;
  if (scrollEnd || $("logFollow").checked) view.scrollTop = view.scrollHeight;
}

function emptyText() {
  if (!["current", "all"].includes(state.runView[state.stream])) return "Nothing logged in that run.";
  return [
    "Server output shows up here once it's launched from Fika Hub.",
    "Headless BepInEx log. Waiting for new lines.",
    "Your game's BepInEx log. Waiting for new lines.",
  ][state.stream];
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
  saveUi();
  $$(".tab").forEach((t) => t.classList.toggle("active", +t.dataset.stream === i));
  state.logSeen[i] = state.logErr[i];
  state.badSeen[i] = state.logBad[i];
  renderRuns();
  renderLog(true);
  updateBadges();
}
$$(".tab").forEach((t) => (t.onclick = () => setStream(+t.dataset.stream)));
$$("#levelSeg button").forEach(
  (b) =>
    (b.onclick = () => {
      state.level = b.dataset.level;
      saveUi();
      $$("#levelSeg button").forEach((x) => x.classList.toggle("on", x === b));
      renderLog(true);
    }),
);
$("logRun").onchange = () => {
  state.runView[state.stream] = $("logRun").value;
  renderLog(true);
};
$("logQuiet").onchange = () => renderLog(true);
let searchT;
$("logSearch").oninput = () => {
  clearTimeout(searchT);
  searchT = setTimeout(() => renderLog(true), 120);
};
$("logWrap").onchange = () => $("logView").classList.toggle("wrap", $("logWrap").checked);
$("logView").onscroll = () => {
  const v = $("logView");
  if (v.scrollHeight - v.scrollTop - v.clientHeight < 40) $("logJump").hidden = true;
};
$("logJump").onclick = () => {
  $("logView").scrollTop = $("logView").scrollHeight;
  $("logJump").hidden = true;
};
$("logCopy").onclick = async () => {
  const t = shownLines()
    .filter((l) => l.lvl !== "gap")
    .map((l) => l.text)
    .join("\n");
  if (!t) return toast("Nothing to copy");
  try {
    await navigator.clipboard.writeText(t);
    toast("Copied what's shown");
  } catch {
    toast("Clipboard unavailable", true);
  }
};
$("logSave").onclick = async () => {
  // always the raw stream: filters are for reading, saved logs are for support
  const t = state.logs[state.stream].map((l) => l.text).join("\n");
  if (!t) return toast("Nothing to save");
  const stamp = new Date().toISOString().slice(0, 19).replace(/[-:T]/g, "");
  if (await call("save_text", { name: `${STREAM_NAMES[state.stream]}-${stamp}.log`, text: t }).catch(() => false))
    toast("Saved the full log");
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
  $("cfgRoots").innerHTML = roots
    .map((r) => `<button data-root="${r.key}" ${r.exists ? "" : "disabled"} title="${esc(r.path)}">${esc(r.label)}</button>`)
    .join("");
  $$("#cfgRoots button").forEach((b) => (b.onclick = () => selectRoot(b.dataset.root)));
  const first = roots.find((r) => r.exists && r.key === state.cfg.root) || roots.find((r) => r.exists);
  if (first) selectRoot(first.key);
}

async function selectRoot(key) {
  if (!(await leaveDirty())) return;
  state.cfg.root = key;
  saveUi();
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
  const pick = (keys) =>
    keys
      .filter((k) => k.startsWith(state.cfg.root + "|"))
      .map((k) => byRel.get(k.slice(state.cfg.root.length + 1)))
      .filter(Boolean);
  const pinned = q ? [] : pick(state.settings.pinnedConfigs || []);
  const recent = q
    ? []
    : pick(state.settings.recentConfigs || [])
        .filter((e) => !isPinned(e.rel))
        .slice(0, 5);

  const groups = {};
  for (const e of list) (groups[e.group] ||= []).push(e);
  const head = [
    ["Pinned", pinned],
    ["Recent", recent],
  ]
    .filter(([, items]) => items.length)
    .map(([g, items]) => `<div class="cfg-group quick-group"><b>${g}</b>${items.map((e) => cfgItem(e, true)).join("")}</div>`)
    .join("");
  $("cfgList").innerHTML = list.length
    ? head +
      Object.entries(groups)
        .map(
          ([g, items]) => `<div class="cfg-group"><b title="${esc(g)}">${esc(g)}</b>${items.map((e) => cfgItem(e, false)).join("")}</div>`,
        )
        .join("")
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
  const root = state.cfg.root;
  let doc;
  try {
    doc = await invoke("config_read", { root, rel });
  } catch (e) {
    return cfgStatus("bad", String(e));
  }
  state.cfg.open = rel;
  state.cfg.openRoot = root;
  const key = cfgKey(rel);
  state.settings.recentConfigs = [key, ...(state.settings.recentConfigs || []).filter((k) => k !== key)].slice(0, 12);
  saveSettings();
  renderPin();
  state.cfg.stamp = doc.stamp;
  state.cfg.crlf = doc.text.includes("\r\n") && !/(^|[^\r])\n/.test(doc.text);
  state.cfg.findLine = null;
  $("cfgFind").value = "";
  $("cfgFindCount").textContent = "";
  state.cfg.original = doc.text.replace(/\r\n?/g, "\n");
  const ta = $("cfgText");
  ta.value = state.cfg.original;
  ta.disabled = !!doc.readOnly;
  state.cfg.readOnly = doc.readOnly || null;
  ta.scrollTop = 0;
  $("cfgName").textContent = rel.split("\\").pop();
  $("cfgPath").textContent = rel;
  $("cfgOpenDir").disabled = false;
  state.cfg.errLine = null;
  $("cfgKeySearch").value = "";
  state.cfg.collapsed = new Set();
  state.cfg.base = plainJson(state.cfg.original);
  updateGutter();
  renderCfgList();
  if (doc.readOnly) cfgStatus("bad", doc.readOnly);
  else validateCfg();
  updateCfgButtons();
  cfgRefresh();
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
  const err = state.cfg.errLine;
  let s = "";
  for (let i = 1; i <= n; i++) s += (i === err ? `<b>${i}</b>` : i) + "\n";
  $("cfgGutter").innerHTML = s + " \n";
}
let valT;
async function validateCfg() {
  if (!state.cfg.open) return;
  try {
    const msg = await invoke("config_validate", { rel: state.cfg.open, text: $("cfgText").value });
    cfgStatus(isDirty() ? "dirty" : "ok", (isDirty() ? "Unsaved · " : "") + msg);
    setErrLine(null);
  } catch (e) {
    const m = String(e).match(/line (\d+), column (\d+)/);
    cfgStatus(
      "bad",
      m
        ? `Line ${m[1]}: ${String(e)
            .replace(/^JSON error at line \d+, column \d+: /, "")
            .replace(/ at line \d+ column \d+$/, "")}`
        : String(e),
    );
    setErrLine(m ? +m[1] : null);
  }
}
$("cfgText").oninput = () => cfgTextChanged(false);
function cfgTextChanged(fromForm) {
  updateGutter();
  updateCfgButtons();
  clearTimeout(valT);
  valT = setTimeout(validateCfg, 250);
  scheduleHl();
  if (!fromForm && state.cfgView === "form") renderForm();
  if (fromForm) paintChanges();
}
$("cfgText").onscroll = syncScroll;
$("cfgText").onkeydown = (e) => {
  if (e.key === "Tab") {
    e.preventDefault();
    const ta = e.target,
      s = ta.selectionStart;
    ta.setRangeText("    ", s, ta.selectionEnd, "end");
    ta.oninput();
  }
};
$("cfgRevert").onclick = () => {
  if (state.cfg.readOnly) return;
  $("cfgText").value = state.cfg.original;
  $("cfgText").oninput();
};
$("cfgOpenDir").onclick = () => call("open_folder", { which: state.cfg.openRoot + "|" + state.cfg.open }).catch(() => {});
async function saveConfig() {
  if (!isDirty() || state.cfg.readOnly) return;
  try {
    const text = $("cfgText").value;
    state.cfg.stamp = await invoke("config_save", {
      root: state.cfg.openRoot,
      rel: state.cfg.open,
      text: state.cfg.crlf ? text.replace(/\n/g, "\r\n") : text,
      stamp: state.cfg.stamp,
    });
    state.cfg.original = text;
    state.cfg.base = plainJson(text);
    paintChanges();
    if (state.cfgView === "form") renderForm();
    updateCfgButtons();
    validateCfg();
    toast("Saved, backup kept");
  } catch (e) {
    cfgStatus("bad", String(e));
    toast("Not saved", true);
  }
}
$("cfgSave").onclick = saveConfig;

// ---- raw view: highlighting, error line, find, format ----
const LINE_H = 20;
const isJsonFile = () => /\.(json|jsonc|json5)$/i.test(state.cfg.open || "");
const isIniFile = () => /\.(cfg|ini|conf|properties|toml)$/i.test(state.cfg.open || "");

// null when the text isn't plain JSON (comments, errors), since the form would rewrite it
function plainJson(text) {
  if (!isJsonFile()) return null;
  try {
    const v = JSON.parse(text);
    return v && typeof v === "object" ? v : null;
  } catch {
    return null;
  }
}
// Number keys get reordered, duplicate keys merged and huge integers rounded by JSON.parse, so the form stays off for those.
// 1.0 vs 1 is fine, it's the same value to the mod.
const canon = (t) =>
  t.replace(/("(?:\\.|[^"\\])*")|(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)|\s+/g, (m, str, num) =>
    str ? str : num ? (/^-?\d+$/.test(num) && !Number.isSafeInteger(Number(num)) ? "#BIG" + num : String(Number(num))) : "",
  );
function roundTrips(text, v) {
  return canon(text) === JSON.stringify(v);
}

const JSON_TOK =
  /("(?:\\.|[^"\\\n])*")(\s*:)?|(\/\/[^\n]*|\/\*[\s\S]*?\*\/)|(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)|\b(true|false|null)\b|([{}[\],:])/g;
function hlJson(t) {
  let out = "",
    last = 0,
    m;
  JSON_TOK.lastIndex = 0;
  while ((m = JSON_TOK.exec(t))) {
    out += esc(t.slice(last, m.index));
    if (m[1]) out += `<span class="${m[2] ? "k" : "s"}">${esc(m[1])}</span>` + (m[2] ? `<span class="p">${esc(m[2])}</span>` : "");
    else if (m[3]) out += `<span class="c">${esc(m[3])}</span>`;
    else if (m[4]) out += `<span class="n">${m[4]}</span>`;
    else if (m[5]) out += `<span class="b">${m[5]}</span>`;
    else out += `<span class="p">${esc(m[6])}</span>`;
    last = JSON_TOK.lastIndex;
  }
  return out + esc(t.slice(last));
}
function hlIni(t) {
  return t
    .split("\n")
    .map((l) => {
      if (/^\s*[#;]/.test(l)) return `<span class="c">${esc(l)}</span>`;
      if (/^\s*\[.*\]\s*$/.test(l)) return `<span class="sec">${esc(l)}</span>`;
      const m = l.match(/^(\s*[^=:\n]+?)(\s*[=:]\s*)(.*)$/);
      if (!m) return esc(l);
      const v = /^(true|false)$/i.test(m[3].trim()) ? "b" : /^-?[\d.]+$/.test(m[3].trim()) ? "n" : "s";
      return `<span class="k">${esc(m[1])}</span><span class="p">${esc(m[2])}</span><span class="${v}">${esc(m[3])}</span>`;
    })
    .join("\n");
}
let hlT;
function scheduleHl() {
  clearTimeout(hlT);
  hlT = setTimeout(paintHl, 40);
}
function paintHl() {
  if (state.cfgView !== "raw") return;
  const t = $("cfgText").value;
  // past ~600 KB the colouring costs more than it helps
  const html = t.length > 600000 ? esc(t) : isJsonFile() ? hlJson(t) : isIniFile() ? hlIni(t) : esc(t);
  $("cfgHl").innerHTML = html + "\n\n";
  syncScroll();
}
function syncScroll() {
  const ta = $("cfgText");
  $("cfgHl").scrollTop = ta.scrollTop;
  $("cfgHl").scrollLeft = ta.scrollLeft;
  $("cfgGutter").scrollTop = ta.scrollTop;
  placeBand("cfgErrBand", state.cfg.errLine);
  placeBand("cfgFindBand", state.cfg.findLine);
}
function placeBand(id, line) {
  const el = $(id);
  el.hidden = !line || state.cfgView !== "raw";
  if (line) el.style.transform = `translateY(${12 + (line - 1) * LINE_H - $("cfgText").scrollTop}px)`;
}
function setErrLine(n) {
  if (state.cfg.errLine === n) return;
  state.cfg.errLine = n;
  updateGutter();
  syncScroll();
}
function scrollToLine(line) {
  const ta = $("cfgText");
  const y = (line - 1) * LINE_H;
  if (y < ta.scrollTop || y > ta.scrollTop + ta.clientHeight - 60) ta.scrollTop = Math.max(0, y - ta.clientHeight / 3);
  syncScroll();
}
const lineOf = (t, i) => t.slice(0, i).split("\n").length;

// "bots › bosses › 2" for wherever the caret sits
function crumbAt(t, pos) {
  const stack = [];
  let i = 0,
    pendingKey = null;
  while (i < pos) {
    const c = t[i];
    if (c === '"') {
      let j = i + 1;
      while (j < t.length && t[j] !== '"') j += t[j] === "\\" ? 2 : 1;
      const str = t.slice(i + 1, j);
      let k = j + 1;
      while (k < t.length && /\s/.test(t[k])) k++;
      if (t[k] === ":" && stack.length && stack[stack.length - 1].obj) stack[stack.length - 1].key = str;
      i = j + 1;
      continue;
    }
    if (c === "/" && t[i + 1] === "/") {
      while (i < pos && t[i] !== "\n") i++;
      continue;
    }
    if (c === "{") stack.push({ obj: true, key: null });
    else if (c === "[") stack.push({ obj: false, idx: 0 });
    else if (c === "}" || c === "]") stack.pop();
    else if (c === "," && stack.length && !stack[stack.length - 1].obj) stack[stack.length - 1].idx++;
    i++;
  }
  return stack
    .map((f) => (f.obj ? f.key : String(f.idx)))
    .filter((x) => x != null)
    .join(" › ");
}
let crumbT;
function updatePos() {
  clearTimeout(crumbT);
  crumbT = setTimeout(() => {
    const ta = $("cfgText"),
      t = ta.value,
      p = ta.selectionStart;
    const before = t.slice(0, p).split("\n");
    $("cfgPos").textContent =
      state.cfg.open && state.cfgView === "raw" ? `Ln ${before.length}, Col ${before[before.length - 1].length + 1}` : "";
    $("cfgCrumb").textContent = state.cfg.open && state.cfgView === "raw" && isJsonFile() && t.length < 600000 ? crumbAt(t, p) : "";
  }, 60);
}
["keyup", "click", "select"].forEach((ev) => $("cfgText").addEventListener(ev, updatePos));

$("cfgFormat").onclick = () => {
  const t = $("cfgText").value;
  let v;
  try {
    v = JSON.parse(t);
  } catch {
    return toast(state.cfg.errLine ? `Fix line ${state.cfg.errLine} first` : "This file has comments, Format would remove them", true);
  }
  if (!roundTrips(t, v)) return toast("Format would reorder keys or rewrite numbers in this file, so it's off here", true);
  const out = JSON.stringify(v, null, indentOf(t)) + (t.endsWith("\n") ? "\n" : "");
  if (out === t) return toast("Already formatted");
  $("cfgText").value = out;
  cfgTextChanged(false);
  toast("Formatted");
};
function indentOf(t) {
  const m = t.match(/\n([ \t]+)["{[\]}]/);
  if (!m) return 4;
  return m[1].includes("\t") ? "\t" : Math.min(m[1].length, 8);
}

function runFind(step) {
  const q = $("cfgFind").value.toLowerCase();
  const t = $("cfgText").value,
    low = t.toLowerCase();
  if (!q) {
    state.cfg.findLine = null;
    $("cfgFindCount").textContent = "";
    return syncScroll();
  }
  const hits = [];
  for (let i = low.indexOf(q); i !== -1 && hits.length < 5000; i = low.indexOf(q, i + q.length)) hits.push(i);
  if (!hits.length) {
    state.cfg.findLine = null;
    $("cfgFindCount").textContent = "No matches";
    return syncScroll();
  }
  const ta = $("cfgText");
  const from = step ? ta.selectionStart + (step > 0 ? 1 : -1) : ta.selectionStart;
  let n = step < 0 ? hits.findLastIndex((h) => h < from) : hits.findIndex((h) => h >= from);
  if (n === -1) n = step < 0 ? hits.length - 1 : 0;
  ta.setSelectionRange(hits[n], hits[n] + q.length);
  state.cfg.findLine = lineOf(t, hits[n]);
  $("cfgFindCount").textContent = `${n + 1} of ${hits.length}`;
  scrollToLine(state.cfg.findLine);
}
$("cfgFind").oninput = () => runFind(0);
$("cfgFind").onkeydown = (e) => {
  if (e.key === "Enter") {
    e.preventDefault();
    runFind(e.shiftKey ? -1 : 1);
  }
  if (e.key === "Escape") {
    const ta = $("cfgText");
    ta.focus();
    state.cfg.findLine = null;
    syncScroll();
  }
};

// ---- form view ----
const getAt = (o, path) => path.reduce((v, k) => (v == null ? undefined : v[k]), o);
function setAt(o, path, val) {
  getAt(o, path.slice(0, -1))[path[path.length - 1]] = val;
}
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const isPrimArr = (v) => Array.isArray(v) && v.every((x) => x === null || typeof x !== "object");

function formCommit() {
  const t = $("cfgText").value;
  const out = JSON.stringify(state.cfg.model, null, indentOf(state.cfg.original)) + (state.cfg.original.endsWith("\n") ? "\n" : "");
  if (out !== t) {
    $("cfgText").value = out;
    cfgTextChanged(true);
  }
}

function countChanges(a, b) {
  if (a && b && typeof a === "object" && typeof b === "object" && !isPrimArr(a)) {
    const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
    let n = 0;
    keys.forEach((k) => (n += countChanges(a[k], b[k])));
    return n;
  }
  return same(a, b) ? 0 : 1;
}
function paintChanges() {
  if (!state.cfg.model || !state.cfg.base) {
    $("cfgChanges").textContent = "";
    return;
  }
  const n = countChanges(state.cfg.base, state.cfg.model);
  $("cfgChanges").textContent = n ? plural(n, "change") + " not saved" : "";
}

function renderForm() {
  const box = $("cfgForm");
  if (!state.cfg.open) {
    box.innerHTML = `<div class="empty">Pick a config on the left.</div>`;
    return;
  }
  const text = $("cfgText").value;
  const parsed = state.cfg.readOnly ? null : plainJson(text);
  const lossy = parsed && !roundTrips(text, parsed);
  const model = lossy ? null : parsed;
  state.cfg.model = model;
  $("cfgKeySearch").disabled = !model;
  if (!model) {
    const why = state.cfg.readOnly
      ? ["This file is read-only here.", state.cfg.readOnly]
      : lossy
        ? [
            "This file has to stay as written.",
            "It has number keys, repeated keys or very large numbers that the form would reorder or round, so it's edited as text.",
          ]
        : !isJsonFile()
          ? ["Form view is for JSON files.", "This one is edited as text."]
          : state.cfg.errLine
            ? [`The JSON has an error on line ${state.cfg.errLine}.`, "Fix it in Raw, then come back."]
            : ["This file has comments.", "The form would strip them on save, so it's edited as text."];
    box.innerHTML = `<div class="f-msg"><b>${esc(why[0])}</b><span>${esc(why[1])}</span><button class="ghost sm" id="fToRaw">Open in Raw</button></div>`;
    $("fToRaw").onclick = () => setCfgView("raw");
    paintChanges();
    return;
  }
  const q = $("cfgKeySearch").value.trim().toLowerCase();
  box.innerHTML = "";
  if (q) {
    const hits = [];
    walkLeaves(model, [], (path) => {
      if (path.join(".").toLowerCase().includes(q)) hits.push(path);
    });
    hits.slice(0, 300).forEach((p) => box.append(fRow(p, p.join(" › "), 0)));
    if (!hits.length) box.innerHTML = `<div class="empty">No keys match.</div>`;
    else if (hits.length > 300)
      box.insertAdjacentHTML("beforeend", `<div class="f-hint">Showing 300 of ${hits.length}. Narrow the search.</div>`);
  } else {
    box.append(fChildren([], 0));
  }
  paintChanges();
}
function walkLeaves(v, path, fn) {
  if (v && typeof v === "object" && !isPrimArr(v))
    for (const k of Object.keys(v)) walkLeaves(v[k], [...path, Array.isArray(v) ? +k : k], fn);
  else if (path.length) fn(path);
}

function fChildren(path, depth) {
  const v = path.length ? getAt(state.cfg.model, path) : state.cfg.model;
  const frag = document.createDocumentFragment();
  const keys = Object.keys(v);
  // big configs open collapsed so the page stays quick
  const openByDefault = keys.length <= 40 && depth < 2;
  for (const k of keys) {
    const p = [...path, Array.isArray(v) ? +k : k];
    const child = v[k];
    const label = Array.isArray(v) ? `#${+k + 1}` : k;
    if (child && typeof child === "object" && !isPrimArr(child)) frag.append(fSection(p, label, depth, openByDefault));
    else frag.append(fRow(p, label, depth));
  }
  if (!keys.length) frag.append(Object.assign(document.createElement("div"), { className: "f-hint", textContent: "Empty" }));
  return frag;
}
function fSection(path, label, depth, openByDefault) {
  const key = JSON.stringify(path);
  const v = getAt(state.cfg.model, path);
  const sec = document.createElement("div");
  sec.className = "f-sec";
  const n = Object.keys(v).length;
  sec.innerHTML = `<button style="padding-left:${18 + depth * 18}px"><i>›</i><span></span><small>${Array.isArray(v) ? plural(n, "item") : plural(n, "key")}</small></button>`;
  sec.querySelector("span").textContent = label;
  const isOpen = () => (openByDefault ? !state.cfg.collapsed.has(key) : state.cfg.collapsed.has("+" + key));
  const paint = () => {
    sec.classList.toggle("open", isOpen());
    sec.querySelector("div")?.remove();
    if (isOpen()) {
      const body = document.createElement("div");
      body.append(fChildren(path, depth + 1));
      sec.append(body);
    }
  };
  sec.firstChild.onclick = () => {
    const k = openByDefault ? key : "+" + key;
    state.cfg.collapsed.has(k) ? state.cfg.collapsed.delete(k) : state.cfg.collapsed.add(k);
    paint();
  };
  paint();
  return sec;
}

function fRow(path, label, depth) {
  const row = document.createElement("div");
  row.className = "f-row";
  row.style.paddingLeft = 18 + depth * 18 + "px";
  const lab = document.createElement("label");
  lab.innerHTML = `<span></span><i class="f-chg" hidden>changed</i>`;
  lab.firstChild.textContent = label;
  lab.title = path.join(" › ");
  row.append(lab);
  const v = getAt(state.cfg.model, path);
  const markChg = () =>
    (lab.querySelector(".f-chg").hidden = !state.cfg.base || same(getAt(state.cfg.base, path), getAt(state.cfg.model, path)));
  const set = (val) => {
    setAt(state.cfg.model, path, val);
    markChg();
    formCommit();
  };

  if (typeof v === "boolean") {
    row.insertAdjacentHTML("beforeend", `<label class="toggle"><input type="checkbox" ${v ? "checked" : ""}><span></span></label>`);
    row.querySelector("input").onchange = (e) => set(e.target.checked);
  } else if (typeof v === "number") {
    const inp = Object.assign(document.createElement("input"), { className: "f-in num", value: String(v), spellcheck: false });
    inp.oninput = () => {
      const n = inp.value.trim() === "" ? NaN : Number(inp.value);
      inp.classList.toggle("bad", !Number.isFinite(n));
      if (Number.isFinite(n)) set(n);
    };
    inp.onblur = () => {
      inp.value = String(getAt(state.cfg.model, path));
      inp.classList.remove("bad");
    };
    row.append(inp);
  } else if (typeof v === "string") {
    const inp = Object.assign(document.createElement("input"), { className: "f-in", value: v, spellcheck: false });
    inp.oninput = () => set(inp.value);
    row.append(inp);
  } else if (v === null) {
    row.insertAdjacentHTML("beforeend", `<span class="f-null">null</span>`);
  } else if (Array.isArray(v)) {
    row.append(fChips(path, set));
  }
  markChg();
  return row;
}
function fChips(path, set) {
  const box = document.createElement("div");
  box.className = "f-chips";
  const paint = () => {
    const arr = getAt(state.cfg.model, path);
    box.innerHTML =
      arr.map((x, i) => `<span class="f-chip"><span></span><button data-i="${i}" title="Remove">×</button></span>`).join("") +
      `<input class="f-add" placeholder="+ add" spellcheck="false">`;
    box.querySelectorAll(".f-chip > span").forEach((el, i) => (el.textContent = JSON.stringify(arr[i]).replace(/^"|"$/g, "")));
    box.querySelectorAll("[data-i]").forEach(
      (b) =>
        (b.onclick = () => {
          set(arr.filter((_, i) => i !== +b.dataset.i));
          paint();
        }),
    );
    const add = box.querySelector(".f-add");
    add.onkeydown = (e) => {
      if (e.key !== "Enter" || !add.value.trim()) return;
      const raw = add.value.trim();
      // keep the list's type: numbers stay numbers, true/false stay booleans
      const first = arr.find((x) => x !== null);
      const kind = first === undefined ? null : typeof first;
      let val;
      if (kind === "number") val = Number.isFinite(Number(raw)) ? Number(raw) : undefined;
      else if (kind === "boolean") val = /^(true|false)$/i.test(raw) ? raw.toLowerCase() === "true" : undefined;
      else if (kind === "string") val = raw;
      else {
        try {
          val = JSON.parse(raw);
          if (val !== null && typeof val === "object") val = raw;
        } catch {
          val = raw;
        }
      }
      if (val === undefined) {
        add.classList.add("bad");
        return;
      }
      set([...arr, val]);
      paint();
      box.querySelector(".f-add").focus();
    };
  };
  paint();
  return box;
}
let keyT;
$("cfgKeySearch").oninput = () => {
  clearTimeout(keyT);
  keyT = setTimeout(renderForm, 150);
};

// ---- view switch ----
state.cfgView = "form";
function setCfgView(v) {
  state.cfgView = v;
  $$("#cfgView button").forEach((b) => b.classList.toggle("on", b.dataset.view === v));
  $("cfgRaw").hidden = v !== "raw";
  $("cfgRawBar").hidden = v !== "raw";
  $("cfgForm").hidden = v !== "form";
  $("cfgFormBar").hidden = v !== "form";
  cfgRefresh();
  saveUi();
}
function cfgRefresh() {
  if (state.cfgView === "form") renderForm();
  else {
    paintHl();
    updatePos();
  }
  updatePos();
}
$$("#cfgView button").forEach((b) => (b.onclick = () => setCfgView(b.dataset.view)));

// ============================ setup check ============================

let checkResolve = null;

async function runChecks(forLaunch) {
  $("checks").hidden = false;
  $("checksTitle").textContent = "Checking setup…";
  $("checkList").innerHTML = `<div class="empty">Looking at installs, ports and profiles…</div>`;
  $("checksLaunch").hidden = true;
  const profile = noGame(state.mode) ? null : state.profileId;
  let list = [];
  try {
    list = await invoke("health_check", { mode: state.mode, profile });
  } catch (e) {
    list = [{ level: "bad", title: String(e) }];
  }
  const bad = list.filter((c) => c.level === "bad").length;
  const warn = list.filter((c) => c.level === "warn").length;
  $("checksTitle").textContent = bad ? `${plural(bad, "problem")} found` : warn ? `Ready, ${plural(warn, "warning")}` : "All good";
  // problems first, the passing checks are just reassurance
  const order = { bad: 0, warn: 1, ok: 2 };
  list.sort((a, b) => order[a.level] - order[b.level]);
  $("checkList").innerHTML = list
    .map(
      (c, i) => `<div class="check-row ${c.level}"><span class="dot"></span>
      <div><b>${esc(c.title)}</b>${c.fix ? `<small>${esc(c.fix)}</small>` : ""}</div>
      ${c.go ? `<button class="ghost sm" data-check-go="${c.go}">Open</button>` : "<span></span>"}</div>`,
    )
    .join("");
  $$("[data-check-go]").forEach(
    (b) =>
      (b.onclick = () => {
        closeChecks(false);
        goto(b.dataset.checkGo);
      }),
  );
  $("checksLaunch").hidden = !forLaunch;
  return { bad, warn };
}

function closeChecks(launch) {
  $("checks").hidden = true;
  if (checkResolve) {
    checkResolve(launch);
    checkResolve = null;
  }
}
$("btnCheck").onclick = () => runChecks(false);
$("checksClose").onclick = () => closeChecks(false);
$("checksLaunch").onclick = () => closeChecks(true);
$("checksRerun").onclick = () => runChecks(!$("checksLaunch").hidden);

/// Resolves true when launch should go ahead. Only stops for real problems.
async function preflight() {
  const { bad } = await runChecks(true);
  if (!bad) {
    $("checks").hidden = true;
    return true;
  }
  return new Promise((r) => (checkResolve = r));
}

// ============================ headless power ============================

$("btnHlPower").onclick = async () => {
  const s = state.status;
  if (s?.headless.owned) {
    const n = hlRaids();
    if (
      n &&
      !(await confirmBox("Stop headless?", `${plural(n, "raid")} running on the headless will end for everyone in them.`, "Stop headless"))
    )
      return;
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
  headless_crash: "headless crash",
  headless_restart: "headless restart",
  headless_stop: "headless stopped",
  server_crash: "server crash",
  server_restart: "server restart",
  game_exit: "game closed",
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
      if (cur && !cur.end) ((cur.end = e.t), (cur.why = "no clean stop recorded"));
      cur = { start: e.t, launch: e, counts: {}, end: null, why: null };
      out.push(cur);
    } else if (e.kind === "stop") {
      if (cur && !cur.end) {
        cur.end = e.t;
        cur.why = e.why;
      }
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
  $("sessionList").innerHTML = sessions.length
    ? sessions
        .map((s, i) => {
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
        })
        .join("")
    : `<div class="empty">No sessions yet. They're recorded from the next launch on.</div>`;
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
    if (dayText !== day) {
      day = dayText;
      rows.push(`<div class="bk-day">${esc(dayText)}</div>`);
    }
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
      rows.push(
        `<div class="bk-detail">${b.profiles
          .map(
            (p) => `<label class="bk-prof">
          <input type="checkbox" data-prof="${esc(p.id)}" ${state.bk.picked.has(p.id) ? "checked" : ""}>
          <span class="bk-check"></span><b>${esc(p.nickname || p.id)}</b>
          <small>Lv ${p.level} · ${bytes(p.size)}${p.headless ? " · headless" : ""}</small></label>`,
          )
          .join("")}</div>`,
      );
    }
  }
  $("backupList").innerHTML = rows.length
    ? rows.join("")
    : `<div class="empty">${list.length ? "None from that source." : "No backups yet. SPT makes its own as you play, and Fika Hub makes one at each launch."}</div>`;

  $$(".bk-row").forEach(
    (el) =>
      (el.onclick = (e) => {
        if (e.target.closest("[data-bk-del]")) return;
        const key = el.dataset.bk;
        if (state.bk.sel === key) return;
        state.bk.sel = key;
        const b = list.find((x) => bkKey(x) === key);
        state.bk.picked = new Set(b.profiles.map((p) => p.id));
        renderBackups();
      }),
  );
  $$("[data-prof]").forEach(
    (c) =>
      (c.onchange = () => {
        c.checked ? state.bk.picked.add(c.dataset.prof) : state.bk.picked.delete(c.dataset.prof);
        paintBkBar();
      }),
  );
  $$("[data-bk-del]").forEach(
    (btn) =>
      (btn.onclick = async () => {
        if (!(await confirmBox("Delete this backup?", "Its copied profiles are removed from disk for good.", "Delete"))) return;
        await call("backup_delete", { id: btn.dataset.bkDel }).catch(() => {});
        loadBackups();
      }),
  );
  paintBkBar();
}

function paintBkBar() {
  const b = state.bk.list.find((x) => bkKey(x) === state.bk.sel);
  const n = b ? b.profiles.filter((p) => state.bk.picked.has(p.id)).length : 0;
  const serverUp = !!state.status?.portOpen;
  $("bkBar").classList.toggle("ready", !!b);
  $("bkSel").innerHTML = !b
    ? "Pick a backup to load"
    : serverUp
      ? `<b>${when(b.created)}</b> · stop the server before loading`
      : `<b>${when(b.created)}</b> · ${plural(n, "profile")} selected`;
  $("bkLoad").disabled = !b || !n || serverUp;
}

$$("#bkFilter button").forEach(
  (btn) =>
    (btn.onclick = () => {
      state.bk.src = btn.dataset.src;
      saveUi();
      $$("#bkFilter button").forEach((x) => x.classList.toggle("on", x === btn));
      renderBackups();
    }),
);

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
  } catch (e) {
    toast(String(e), true);
  }
  state.bk.sel = null;
  loadBackups();
  refreshProfiles();
};

$("histRefresh").onclick = loadHistory;
$("bkNow").onclick = async () => {
  try {
    const b = await invoke("backup_create");
    toast(b ? "Profiles backed up" : "Nothing changed since the last backup");
  } catch (e) {
    toast(String(e), true);
  }
  loadBackups();
};
$("bkOpen").onclick = () => call("backup_open").catch(() => {});
$("diagExport").onclick = async () => {
  $("diagExport").disabled = true;
  try {
    if (await invoke("export_diagnostics")) toast("Diagnostics saved");
  } catch (e) {
    toast(String(e), true);
  }
  $("diagExport").disabled = false;
};

// ============================ diff ============================

/// Line diff via LCS on the middle part, after trimming the shared head and tail.
function diffLines(a, b) {
  const A = a.split("\n"),
    B = b.split("\n");
  let s = 0;
  while (s < A.length && s < B.length && A[s] === B[s]) s++;
  let ea = A.length,
    eb = B.length;
  while (ea > s && eb > s && A[ea - 1] === B[eb - 1]) {
    ea--;
    eb--;
  }
  const out = A.slice(0, s).map((t) => ["=", t]);
  const n = ea - s,
    m = eb - s;
  if (n * m > 4e6) {
    // too big to align, a plain before/after is still readable
    A.slice(s, ea).forEach((t) => out.push(["-", t]));
    B.slice(s, eb).forEach((t) => out.push(["+", t]));
  } else {
    const L = new Uint32Array((n + 1) * (m + 1));
    for (let i = n - 1; i >= 0; i--)
      for (let j = m - 1; j >= 0; j--)
        L[i * (m + 1) + j] =
          A[s + i] === B[s + j] ? L[(i + 1) * (m + 1) + j + 1] + 1 : Math.max(L[(i + 1) * (m + 1) + j], L[i * (m + 1) + j + 1]);
    let i = 0,
      j = 0;
    while (i < n || j < m) {
      if (i < n && j < m && A[s + i] === B[s + j]) {
        out.push(["=", A[s + i]]);
        i++;
        j++;
      } else if (j < m && (i === n || L[i * (m + 1) + j + 1] > L[(i + 1) * (m + 1) + j])) {
        out.push(["+", B[s + j]]);
        j++;
      } else {
        out.push(["-", A[s + i]]);
        i++;
      }
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
    if (op !== "=") {
      html.push(`<div class="${op === "+" ? "add" : "del"}">${op} ${esc(t)}</div>`);
      continue;
    }
    // collapse long unchanged stretches, keep a few lines around each change
    let e = k;
    while (e < ops.length && ops[e][0] === "=") e++;
    const runLen = e - k;
    const head = k === 0 ? 0 : CTX,
      tail = e === ops.length ? 0 : CTX;
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
  openDrawer(
    `Snapshots · ${label} configs`,
    `<div class="drawer-side">
      <div class="snap-new"><input class="text-in" id="snapName" placeholder="Name, e.g. Before SVM tweak"><button class="primary sm" id="snapTake">Take</button></div>
      <div id="snapList">${
        snaps.length
          ? snaps
              .map(
                (s) =>
                  `<div class="pick" data-snap="${s.id}"><b>${esc(s.name)}</b><small>${when(s.created)} · ${s.files.length} files</small></div>`,
              )
              .join("")
          : `<div class="empty">No snapshots yet. Take one before changing a lot of settings.</div>`
      }</div>
    </div><div class="drawer-main" id="snapMain"><div class="empty">Pick a snapshot to compare it with the files as they are now.</div></div>`,
  );
  $("snapTake").onclick = async () => {
    $("snapTake").disabled = true;
    try {
      const s = await invoke("snapshot_create", { root, name: $("snapName").value });
      toast(`Saved ${s.files.length} files`);
      openSnapshots(s.id);
    } catch (e) {
      toast(String(e), true);
      $("snapTake").disabled = false;
    }
  };
  $$("[data-snap]").forEach(
    (el) =>
      (el.onclick = () =>
        showSnapshot(
          root,
          snaps.find((s) => s.id === el.dataset.snap),
        )),
  );
  const pre = snaps.find((s) => s.id === selectId);
  if (pre) showSnapshot(root, pre);
}

async function showSnapshot(root, snap) {
  $$("[data-snap]").forEach((el) => el.classList.toggle("sel", el.dataset.snap === snap.id));
  const main = $("snapMain");
  main.innerHTML = `<div class="empty">Comparing…</div>`;
  let changes;
  try {
    changes = await invoke("snapshot_compare", { root, id: snap.id });
  } catch (e) {
    main.innerHTML = `<div class="empty">${esc(e)}</div>`;
    return;
  }
  const restorable = changes.filter((c) => c.status !== "added");
  main.innerHTML = `<div class="drawer-bar"><b>${esc(snap.name)}</b><span class="muted small">${when(snap.created)}</span><div class="spacer"></div>
      <button class="ghost sm" id="snapDel">Delete</button>
      <button class="primary sm" id="snapRestoreAll" ${restorable.length ? "" : "disabled"}>Restore ${restorable.length ? plural(restorable.length, "file") : "nothing"}</button></div>
    ${
      changes.length
        ? changes
            .map(
              (c) =>
                `<div class="change" data-rel="${esc(c.rel)}" data-status="${c.status}"><span>${esc(c.rel)}</span><i class="mod-tag ${c.status === "changed" ? "warn" : c.status === "missing" ? "bad" : ""}">${CHANGE_TEXT[c.status]}</i></div>`,
            )
            .join("")
        : `<div class="empty">Everything matches this snapshot.</div>`
    }
    <div id="snapDiff"></div>`;
  $("snapDel").onclick = async () => {
    if (!(await confirmBox("Delete snapshot?", `${snap.name} is removed for good. Your current configs aren't touched.`, "Delete"))) return;
    await call("snapshot_delete", { root, id: snap.id }).catch(() => {});
    openSnapshots();
  };
  $("snapRestoreAll").onclick = async () => {
    if (
      !(await confirmBox(
        "Restore snapshot?",
        `${plural(restorable.length, "file")} go back to how they were in "${snap.name}". Each current file is backed up first. Files added since stay as they are.`,
        "Restore",
      ))
    )
      return;
    await restoreSnap(root, snap, null);
  };
  $$(".change").forEach(
    (el) =>
      (el.onclick = async () => {
        $$(".change").forEach((x) => x.classList.toggle("sel", x === el));
        const rel = el.dataset.rel,
          st = el.dataset.status;
        const old = st === "added" ? "" : await invoke("snapshot_read", { root, id: snap.id, rel }).catch(() => "");
        const now = st === "missing" ? "" : (await invoke("config_read", { root, rel }).catch(() => ({ text: "" }))).text;
        $("snapDiff").innerHTML =
          `<div class="drawer-bar"><b>${esc(rel)}</b><span class="muted small">red is the snapshot, green is now</span><div class="spacer"></div>
        ${st !== "added" ? `<button class="ghost sm" id="snapRestoreOne">Restore this file</button>` : ""}</div>${diffHtml(old, now)}`;
        const one = $("snapRestoreOne");
        if (one) one.onclick = () => restoreSnap(root, snap, [rel]);
      }),
  );
}

async function restoreSnap(root, snap, only) {
  if (isDirty() && state.cfg.openRoot === root && (!only || only.includes(state.cfg.open)) && !(await leaveDirty())) return;
  try {
    const n = await invoke("snapshot_restore", { root, id: snap.id, only });
    toast(n ? `Restored ${plural(n, "file")}` : "Already matched");
    if (state.status?.server.running && root === "server") toast("Restart the server for server configs to apply");
  } catch (e) {
    toast(String(e), true);
  }
  if (state.cfg.open && state.cfg.openRoot === root) openConfig(state.cfg.open);
  showSnapshot(root, snap);
}
$("cfgSnapBtn").onclick = () => openSnapshots();

async function openFileHistory() {
  const root = state.cfg.openRoot,
    rel = state.cfg.open;
  if (!rel) return;
  const [baks, snaps] = await Promise.all([
    invoke("config_backups", { root, rel }).catch(() => []),
    invoke("snapshot_list", { root }).catch(() => []),
  ]);
  const inSnaps = snaps.filter((s) => s.files.includes(rel));
  const items = [
    ...baks.map((b) => ({
      key: "b:" + b.name,
      title: when(b.saved),
      sub: `Before a save · ${bytes(b.size)}`,
      load: () => invoke("config_backup_read", { root, rel, name: b.name }),
    })),
    ...inSnaps.map((s) => ({
      key: "s:" + s.id,
      title: s.name,
      sub: `Snapshot · ${when(s.created)}`,
      load: () => invoke("snapshot_read", { root, id: s.id, rel }),
    })),
  ];
  openDrawer(
    `History · ${rel.split("\\").pop()}`,
    `<div class="drawer-side">${
      items.length
        ? items
            .map((it) => `<div class="pick" data-hist="${esc(it.key)}"><b>${esc(it.title)}</b><small>${esc(it.sub)}</small></div>`)
            .join("")
        : `<div class="empty">No earlier versions. One is kept each time you save here.</div>`
    }</div>
    <div class="drawer-main" id="histMain"><div class="empty">Pick a version to compare it with what's in the editor.</div></div>`,
  );
  $$("[data-hist]").forEach(
    (el) =>
      (el.onclick = async () => {
        $$("[data-hist]").forEach((x) => x.classList.toggle("sel", x === el));
        const it = items.find((x) => x.key === el.dataset.hist);
        let text;
        try {
          text = await it.load();
        } catch (e) {
          $("histMain").innerHTML = `<div class="empty">${esc(e)}</div>`;
          return;
        }
        $("histMain").innerHTML =
          `<div class="drawer-bar"><b>${esc(it.title)}</b><span class="muted small">red is this version, green is the editor</span><div class="spacer"></div>
        <button class="primary sm" id="histLoad">Load into editor</button></div>${diffHtml(text, $("cfgText").value)}`;
        $("histLoad").onclick = () => {
          if (state.cfg.readOnly) return toast("This file is read-only here", true);
          // lands as an unsaved edit, so nothing changes on disk until Save
          $("cfgText").value = text;
          $("cfgText").oninput();
          $("drawer").hidden = true;
          toast("Loaded. Save to keep it, Revert to drop it");
        };
      }),
  );
}
$("cfgHistBtn").onclick = openFileHistory;

// ============================ settings values ============================

function applySettingValues() {
  $$("[data-set-val]").forEach((i) => {
    if (document.activeElement !== i) i.value = state.settings[i.dataset.setVal] ?? "";
  });
}
$$("[data-set-val]").forEach(
  (i) =>
    (i.onchange = async () => {
      const k = i.dataset.setVal;
      state.settings[k] = i.type === "number" ? Math.max(+i.min || 0, Math.min(+i.max || 1e9, Math.round(+i.value || 0))) : i.value.trim();
      i.value = state.settings[k];
      await saveSettings();
      loadNextRestart();
    }),
);

async function loadNextRestart() {
  const t = await invoke("next_restart").catch(() => 0);
  $("nextRestartText").textContent = t ? `Next restart ${when(t)}.` : "Local time on this PC.";
}

$("discordTest").onclick = async () => {
  $("discordTest").disabled = true;
  try {
    await invoke("discord_test");
    toast("Posted to Discord");
  } catch (e) {
    toast(String(e), true);
  }
  $("discordTest").disabled = false;
};

boot();
