const invoke = window.__TAURI__?.core?.invoke;
const ROW_HEIGHT = 38;
const MAX_SCROLL_HEIGHT = 16_000_000;
const OVERSCAN = 5;

const elements = {
  loadFiles: document.querySelector("#loadFiles"),
  loadDemo: document.querySelector("#loadDemo"),
  keyword: document.querySelector("#keyword"),
  searchOptions: document.querySelector(".search-options"),
  level: document.querySelector("#levelFilter"),
  source: document.querySelector("#sourceFilter"),
  viewport: document.querySelector("#listViewport"),
  space: document.querySelector("#virtualSpace"),
  rows: document.querySelector("#virtualRows"),
  empty: document.querySelector("#emptyState"),
  total: document.querySelector("#totalCount"),
  matched: document.querySelector("#matchedCount"),
  loadTime: document.querySelector("#loadTime"),
  filterTime: document.querySelector("#filterTime"),
  rendered: document.querySelector("#renderedCount"),
  fileSummary: document.querySelector("#fileSummary"),
  status: document.querySelector("#statusText"),
  statusDot: document.querySelector("#statusDot"),
};

let totalCount = 0;
let matchedCount = 0;
let datasetVersion = 0;
let sources = [];
let pageStart = -1;
let pageRows = [];
let pageRequest = 0;
let pendingPageStart = null;
let filterRequest = 0;
let loadRequest = 0;
let scrollFrame = 0;
let filterTimer = 0;

function formatCount(value) {
  return new Intl.NumberFormat("zh-CN").format(value);
}

function setBusy(busy, message) {
  elements.loadFiles.disabled = busy;
  elements.loadDemo.disabled = busy;
  elements.statusDot.classList.toggle("busy", busy);
  if (message) elements.status.textContent = message;
}

function renderSources(selectedIndex = -1) {
  elements.source.replaceChildren(new Option("两个文件", "-1"));
  sources.forEach((name, index) => {
    elements.source.add(new Option(name, String(index)));
  });
  elements.source.value = String(selectedIndex);
  elements.fileSummary.textContent = sources.length
    ? sources.join("  +  ")
    : "等待载入两个文件";
  elements.fileSummary.title = elements.fileSummary.textContent;
}

function updateCounts() {
  elements.total.textContent = formatCount(totalCount);
  elements.matched.textContent = formatCount(matchedCount);
}

function visibleStart() {
  if (!matchedCount) return 0;
  const totalHeight = matchedCount * ROW_HEIGHT;
  const virtualHeight = Math.min(totalHeight, MAX_SCROLL_HEIGHT);
  const maxScroll = Math.max(0, virtualHeight - elements.viewport.clientHeight);
  const dataRange = Math.max(0, totalHeight - elements.viewport.clientHeight);
  if (!maxScroll || !dataRange) return 0;
  const progress = Math.min(1, elements.viewport.scrollTop / maxScroll);
  const visibleRows = Math.ceil(elements.viewport.clientHeight / ROW_HEIGHT);
  const maxStart = Math.max(0, matchedCount - visibleRows);
  return Math.min(maxStart, Math.floor(progress * maxStart));
}

function appendHighlighted(node, text, ranges) {
  const value = String(text ?? "");
  if (!ranges.length) {
    node.textContent = value;
    return;
  }
  const characters = Array.from(value);
  let cursor = 0;
  for (const [start, end] of ranges) {
    node.append(document.createTextNode(characters.slice(cursor, start).join("")));
    const mark = document.createElement("mark");
    mark.textContent = characters.slice(start, end).join("");
    node.append(mark);
    cursor = end;
  }
  node.append(document.createTextNode(characters.slice(cursor).join("")));
}

function makeRow(record) {
  const row = document.createElement("div");
  row.className = "log-row";
  row.setAttribute("role", "listitem");

  const time = document.createElement("time");
  appendHighlighted(time, record.timestamp, record.timestampHighlightRanges);
  row.append(time);

  const level = document.createElement("span");
  level.className = `level ${record.level.toLowerCase()}`;
  level.textContent = ({ INFO: "信息", WARN: "警告", ERROR: "错误", DEBUG: "调试", TRACE: "跟踪" })[record.level] ?? record.level;
  row.append(level);

  const source = document.createElement("span");
  source.className = "source";
  source.title = record.source;
  source.textContent = record.source;
  row.append(source);

  const message = document.createElement("span");
  message.className = "message";
  message.title = record.message;
  appendHighlighted(message, record.message, record.highlightRanges);
  row.append(message);
  return row;
}

function renderRows() {
  const start = visibleStart();
  const viewRows = Math.ceil(elements.viewport.clientHeight / ROW_HEIGHT);
  const requestedStart = Math.max(0, start - OVERSCAN);
  const requestedEnd = Math.min(matchedCount, start + viewRows + OVERSCAN);
  const pageEnd = pageStart + pageRows.length;

  elements.space.style.height = `${Math.min(matchedCount * ROW_HEIGHT, MAX_SCROLL_HEIGHT)}px`;
  elements.empty.hidden = matchedCount !== 0;
  elements.rows.style.transform = `translateY(${Math.max(0, elements.viewport.scrollTop - OVERSCAN * ROW_HEIGHT)}px)`;

  if (requestedStart < pageStart || requestedEnd > pageEnd) {
    if (pendingPageStart !== requestedStart) void fetchPage(requestedStart);
  }

  elements.rows.replaceChildren();
  const localStart = Math.max(0, start - OVERSCAN - pageStart);
  const localEnd = Math.min(pageRows.length, start + viewRows + OVERSCAN - pageStart);
  for (let index = localStart; index < localEnd; index++) {
    elements.rows.append(makeRow(pageRows[index]));
  }
  elements.rendered.textContent = String(Math.max(0, localEnd - localStart));
}

async function fetchPage(start) {
  if (!invoke || matchedCount === 0) return;
  const end = pageStart + pageRows.length;
  const viewRows = Math.ceil(elements.viewport.clientHeight / ROW_HEIGHT);
  if (start >= pageStart && start + viewRows + OVERSCAN * 2 <= end) return;
  const requestId = ++pageRequest;
  const filterId = filterRequest;
  pendingPageStart = start;
  try {
    const rows = await invoke("get_logs_page", {
      start,
      limit: Math.max(256, viewRows + OVERSCAN * 4),
    });
    if (requestId !== pageRequest || filterId !== filterRequest) return;
    pendingPageStart = null;
    pageStart = start;
    pageRows = rows;
    renderRows();
  } catch (error) {
    if (requestId === pageRequest) {
      pendingPageStart = null;
      setBusy(false, `读取日志失败：${error}`);
    }
  }
}

async function applyFilters(resetScroll = true) {
  if (!invoke || totalCount === 0) return;
  const requestId = ++filterRequest;
  setBusy(true, "正在按条件筛选日志…");
  try {
    const result = await invoke("filter_logs", {
      keyword: elements.keyword.value,
      matchCase: searchOptions.matchCase,
      wholeWord: searchOptions.wholeWord,
      useRegex: searchOptions.useRegex,
      level: elements.level.value,
      sourceIndex: Number(elements.source.value),
      datasetVersion,
      revision: requestId,
    });
    if (requestId !== filterRequest) return;
    matchedCount = result.matchedCount;
    elements.filterTime.textContent = `${result.filterMs} ms`;
    elements.keyword.removeAttribute("aria-invalid");
    updateCounts();
    pageRequest++;
    pendingPageStart = null;
    pageStart = -1;
    pageRows = [];
    if (resetScroll) elements.viewport.scrollTop = 0;
    renderRows();
    setBusy(false, `筛选完成：${formatCount(matchedCount)} 条结果，耗时 ${result.filterMs} ms`);
  } catch (error) {
    elements.keyword.setAttribute("aria-invalid", "true");
    setBusy(false, `筛选失败：${error}`);
  }
}

async function loadDemo() {
  if (!invoke) {
    setBusy(false, "请在 Tauri 桌面窗口中运行此页面");
    return;
  }
  const requestId = ++loadRequest;
  setBusy(true, "正在生成并合并两个模拟日志文件…");
  try {
    const result = await invoke("generate_demo_logs");
    if (requestId !== loadRequest) return;
    showLoadedData(result);
  } catch (error) {
    setBusy(false, `载入演示数据失败：${error}`);
  }
}

async function chooseFiles() {
  if (!invoke) {
    setBusy(false, "请在 Tauri 桌面窗口中选择本地日志文件");
    return;
  }
  try {
    const paths = await invoke("pick_log_files");
    if (!paths) return;
    if (paths.length !== 2) {
      setBusy(false, "请一次选择两个日志文件");
      return;
    }
    const requestId = ++loadRequest;
    setBusy(true, "正在读取、排序并合并两个文件…");
    const result = await invoke("load_log_files", { paths });
    if (requestId !== loadRequest) return;
    showLoadedData(result);
  } catch (error) {
    setBusy(false, `导入失败：${error}`);
  }
}

function showLoadedData(result) {
  filterRequest++;
  pageRequest++;
  pendingPageStart = null;
  datasetVersion = result.datasetVersion;
  elements.keyword.value = "";
  elements.keyword.removeAttribute("aria-invalid");
  elements.level.value = "ALL";
  totalCount = result.totalCount;
  matchedCount = result.totalCount;
  sources = result.sources;
  pageStart = -1;
  pageRows = [];
  renderSources();
  updateCounts();
  elements.loadTime.textContent = `${result.loadMs} ms`;
  elements.filterTime.textContent = "—";
  elements.viewport.scrollTop = 0;
  elements.empty.textContent = totalCount ? "正在读取当前页…" : "没有可显示的有效日志行";
  elements.empty.hidden = totalCount !== 0;
  setBusy(false, `${result.algorithm} · 已合并 ${formatCount(totalCount)} 条${result.skippedLines ? `，跳过 ${formatCount(result.skippedLines)} 条无效行` : ""}`);
  renderRows();
}

elements.loadDemo.addEventListener("click", () => void loadDemo());
elements.loadFiles.addEventListener("click", () => void chooseFiles());
elements.level.addEventListener("change", () => void applyFilters());
elements.source.addEventListener("change", () => void applyFilters());
elements.keyword.addEventListener("input", () => {
  clearTimeout(filterTimer);
  filterTimer = setTimeout(() => void applyFilters(), 250);
});
const searchOptions = { matchCase: false, wholeWord: false, useRegex: false };
elements.searchOptions.addEventListener("click", event => {
  const button = event.target.closest("button[data-option]");
  if (!button) return;
  const option = button.dataset.option;
  searchOptions[option] = !searchOptions[option];
  button.setAttribute("aria-pressed", String(searchOptions[option]));
  void applyFilters();
});
elements.viewport.addEventListener("scroll", () => {
  if (scrollFrame) return;
  scrollFrame = requestAnimationFrame(() => {
    scrollFrame = 0;
    renderRows();
  });
});
window.addEventListener("resize", renderRows);

void loadDemo();
