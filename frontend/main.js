const invoke = window.__TAURI__?.core?.invoke;
const appWindow = window.__TAURI__?.window?.getCurrentWindow();
const ROW_HEIGHT = 16;
const MAX_SCROLL_HEIGHT = 16_000_000;
const OVERSCAN = 5;
const DEFAULT_COLUMN_ORDER = ["timestamp", "level", "source", "message"];
const COLUMN_ORDER_KEY = "logstryx.columnOrder";
const COLUMN_VISIBILITY_KEY = "logstryx.visibleColumns";
const READING_RULE_KEY = "logstryx.readingPattern";
const READING_RULES_KEY = "logstryx.readingRules";
const FILE_RULES_KEY = "logstryx.fileRules";
const KNOWN_PRESETS_KEY = "logstryx.knownReadingPresets";
const LEGACY_COMBINED_PATTERN = String.raw`^\[?(?P<time>\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}:\d{2}(?:\.\d+)?Z?|\d{2}:\d{2}:\d{2}:\d{3}[+-]\d{4}[ \t]+\d{4}-\d{2}-\d{2})[ \t]+(?:\[?(?P<level>TRACE|DEBUG|INFO|WARN|WARNING|ERROR|FATAL)(?:\][ \t]*|[ \t]+|$))?(?P<message>.*)$`;
const LEGACY_READING_PATTERN = String.raw`^(?P<time>\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}:\d{2}(?:\.\d+)?Z?)[ \t]+(?:\[?(?P<level>TRACE|DEBUG|INFO|WARN|WARNING|ERROR|FATAL)\]?[ \t]+)?(?P<message>.*)$`;
const COLUMN_NAMES = { timestamp: "时间", level: "级别", source: "文件", message: "日志内容" };

const elements = {
  searchField: document.querySelector(".search-field"),
  searchExpand: document.querySelector(".search-expand"),
  keyword: document.querySelector("#keyword"),
  filterKeyword: document.querySelector("#filterKeyword"),
  searchClose: document.querySelector(".search-close"),
  searchOptions: document.querySelector(".search-options"),
  level: document.querySelector("#levelFilter"),
  source: document.querySelector("#sourceFilter"),
  viewport: document.querySelector("#listViewport"),
  space: document.querySelector("#virtualSpace"),
  rows: document.querySelector("#virtualRows"),
  empty: document.querySelector("#emptyState"),
  status: document.querySelector("#statusText"),
  statusDot: document.querySelector("#statusDot"),
  header: document.querySelector(".list-header"),
  panel: document.querySelector(".log-panel"),
  columnSettings: document.querySelector("#columnSettings"),
  columnSettingsOptions: document.querySelector("#columnSettingsOptions"),
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
let searchRequest = 0;
let loadRequest = 0;
let scrollFrame = 0;
let filterTimer = 0;
let searchTimer = 0;
const levels = ["INFO", "WARN", "ERROR", "DEBUG", "TRACE"];
const levelNames = ["信息", "警告", "错误", "调试", "跟踪"];
let selectedLevels = new Set(levels);
let selectedSources = new Set();

function formatCount(value) {
  return new Intl.NumberFormat("zh-CN").format(value);
}

function setBusy(busy, message) {
  elements.statusDot.classList.toggle("busy", busy);
  if (message) elements.status.textContent = message;
}

const readingDialog = document.querySelector("#readingRules");
const readingPattern = document.querySelector("#readingPattern");
const readingSample = document.querySelector("#readingSample");
const readingFeedback = document.querySelector("#readingRuleFeedback");
const readingPreview = document.querySelector("#readingRulePreview");
const readingRuleList = document.querySelector("#readingRuleList");
const readingRuleName = document.querySelector("#readingRuleName");
const readingTimeFormat = document.querySelector("#readingTimeFormat");
const fileRulesDialog = document.querySelector("#fileRules");
const fileAssignments = document.querySelector("#fileRuleAssignments");
const fileFeedback = document.querySelector("#fileRuleFeedback");
const confirmFileRules = document.querySelector("#confirmFileRules");
let draftRules = [];
let selectedRuleId = null;
let pendingPaths = null;
let fileRuleRequest = 0;
let importing = false;
let resumeFileRules = false;
let readingRuleVersion = 0;
const readingRuleReady = (async () => {
  if (!invoke) return null;
  const defaults = await invoke("default_reading_rules");
  const saved = localStorage.getItem(READING_RULES_KEY);
  let rules = saved === null ? structuredClone(defaults) : JSON.parse(saved);
  if (!Array.isArray(rules) || !rules.length || new Set(rules.map(rule => rule.id)).size !== rules.length
    || !rules.every(rule => ["id", "name", "pattern", "timeFormat", "sample"].every(key => typeof rule[key] === "string")
      && rule.id.trim() && rule.name.trim())) {
    throw new Error("已保存的规则列表无效");
  }
  const legacy = localStorage.getItem(READING_RULE_KEY);
  const knownPresets = JSON.parse(localStorage.getItem(KNOWN_PRESETS_KEY) ?? '["date-first","jabra-sdk"]');
  if (!Array.isArray(knownPresets) || !knownPresets.every(id => typeof id === "string")) {
    throw new Error("已保存的内置规则版本无效");
  }
  for (const preset of defaults) {
    if (!knownPresets.includes(preset.id) && !rules.some(rule => rule.id === preset.id)) {
      rules.push(structuredClone(preset));
    }
  }
  if (saved === null && legacy !== null) {
    if (![LEGACY_READING_PATTERN, LEGACY_COMBINED_PATTERN].includes(legacy)) {
      rules.push({ ...defaults[0], id: crypto.randomUUID(), name: "迁移的自定义规则（请核对时间格式）", pattern: legacy, sample: "" });
    }
  }
  localStorage.setItem(READING_RULES_KEY, JSON.stringify(rules));
  localStorage.setItem(KNOWN_PRESETS_KEY, JSON.stringify(defaults.map(rule => rule.id)));
  return { defaults, rules };
})();
// Keep initialization failures observable when the settings or importer is opened.
void readingRuleReady.catch(error => setBusy(false, `恢复读取规则失败：${error}`));

async function showReadingRules() {
  try {
    const state = await readingRuleReady;
    if (!state) throw new Error("请在 Tauri 桌面窗口中配置读取规则");
    draftRules = structuredClone(state.rules);
    selectedRuleId = draftRules[0].id;
    renderRuleList();
    showSelectedRule();
    readingDialog.showModal();
    readingPattern.focus();
  } catch (error) {
    setBusy(false, `打开读取规则失败：${error}`);
  }
}

function currentDraftRule() {
  return draftRules.find(rule => rule.id === selectedRuleId);
}

function renderRuleList() {
  readingRuleList.replaceChildren();
  for (const rule of draftRules) {
    const option = document.createElement("option");
    option.value = rule.id;
    option.textContent = rule.name || "未命名规则";
    readingRuleList.append(option);
  }
  readingRuleList.value = selectedRuleId;
  document.querySelector("#deleteReadingRule").disabled = draftRules.length === 1;
}

function showSelectedRule() {
  const rule = currentDraftRule();
  readingRuleName.value = rule.name;
  readingPattern.value = rule.pattern;
  readingTimeFormat.value = rule.timeFormat;
  readingSample.value = rule.sample;
  readingRuleVersion++;
  readingFeedback.textContent = "";
  readingFeedback.classList.remove("error");
  readingPreview.replaceChildren();
  readingPattern.removeAttribute("aria-invalid");
}

function updateDraftRule() {
  Object.assign(currentDraftRule(), {
    name: readingRuleName.value,
    pattern: readingPattern.value,
    timeFormat: readingTimeFormat.value,
    sample: readingSample.value,
  });
}

for (const input of [readingRuleName, readingPattern, readingTimeFormat, readingSample]) {
  input.addEventListener("input", () => {
    updateDraftRule();
    if (input === readingRuleName) renderRuleList();
    readingRuleVersion++;
    readingFeedback.textContent = "";
    readingPreview.replaceChildren();
    readingPattern.removeAttribute("aria-invalid");
  });
}
async function checkReadingRule(save) {
  const version = ++readingRuleVersion;
  updateDraftRule();
  const rule = structuredClone(currentDraftRule());
  readingFeedback.classList.remove("error");
  readingFeedback.textContent = "正在解析…";
  readingPreview.replaceChildren();
  try {
    if (!invoke) throw new Error("请在 Tauri 桌面窗口中运行");
    if (save) {
      await invoke("validate_reading_rules", { rules: draftRules });
    } else if (!rule.sample.trim()) {
      throw new Error("请先输入示例日志以预览");
    }
    const records = rule.sample.trim() ? await invoke("preview_reading_rule", { rule }) : [];
    if (version !== readingRuleVersion || !readingDialog.open) return;
    readingPattern.removeAttribute("aria-invalid");
    for (const record of records.slice(0, 20)) {
      const row = document.createElement("pre");
      row.textContent = `${record.timestamp}  ${record.level}\n${record.message}`;
      readingPreview.append(row);
    }
    readingFeedback.textContent = `识别到 ${records.length} 条记录${records.length > 20 ? "（预览前 20 条）" : ""}`;
    if (save) {
      const state = await readingRuleReady;
      if (version !== readingRuleVersion || !readingDialog.open) return;
      localStorage.setItem(READING_RULES_KEY, JSON.stringify(draftRules));
      state.rules = structuredClone(draftRules);
      readingDialog.close();
      setBusy(false, "日志读取规则已保存，下次导入文件时生效");
    }
  } catch (error) {
    if (version !== readingRuleVersion) return;
    readingFeedback.classList.add("error");
    readingFeedback.textContent = `规则验证或保存失败：${error}`;
    readingPattern.setAttribute("aria-invalid", "true");
  }
}
document.querySelector("#readingRulesForm").addEventListener("submit", event => {
  event.preventDefault();
  void checkReadingRule(true);
});
document.querySelector("#previewReadingRule").addEventListener("click", () => void checkReadingRule(false));
document.querySelector("#cancelReadingRule").addEventListener("click", () => readingDialog.close());
readingDialog.addEventListener("close", () => {
  readingRuleVersion++;
  if (resumeFileRules && pendingPaths) {
    resumeFileRules = false;
    void showFileRules(pendingPaths);
  }
});
readingRuleList.addEventListener("change", () => {
  selectedRuleId = readingRuleList.value;
  showSelectedRule();
});
document.querySelector("#addReadingRule").addEventListener("click", async () => {
  const state = await readingRuleReady;
  const rule = { ...state.defaults[0], id: crypto.randomUUID(), name: "新规则", sample: "" };
  draftRules.push(rule);
  selectedRuleId = rule.id;
  renderRuleList();
  showSelectedRule();
  readingRuleName.focus();
});
document.querySelector("#deleteReadingRule").addEventListener("click", () => {
  if (draftRules.length <= 1) return;
  draftRules = draftRules.filter(rule => rule.id !== selectedRuleId);
  selectedRuleId = draftRules[0].id;
  renderRuleList();
  showSelectedRule();
});
document.querySelector("#resetReadingRule").addEventListener("click", async () => {
  const state = await readingRuleReady;
  const rule = currentDraftRule();
  const preset = state.defaults.find(item => item.id === rule.id) ?? state.defaults[0];
  Object.assign(rule, structuredClone(preset), { id: rule.id });
  renderRuleList();
  showSelectedRule();
});

async function showFileRules(paths) {
  const request = ++fileRuleRequest;
  pendingPaths = paths;
  fileAssignments.replaceChildren();
  fileFeedback.textContent = "正在为每个文件推荐规则…";
  confirmFileRules.disabled = true;
  if (!fileRulesDialog.open) fileRulesDialog.showModal();
  try {
    const state = await readingRuleReady;
    const suggestions = await invoke("suggest_reading_rules", { paths, rules: state.rules });
    if (request !== fileRuleRequest || !fileRulesDialog.open) return;
    const saved = JSON.parse(localStorage.getItem(FILE_RULES_KEY) ?? "{}");
    if (!saved || typeof saved !== "object" || Array.isArray(saved)) throw new Error("已保存的文件规则绑定无效");
    for (const [index, suggestion] of suggestions.entries()) {
      const row = document.createElement("div");
      row.className = "file-rule-assignment";
      const label = document.createElement("label");
      label.htmlFor = `file-rule-${index}`;
      label.textContent = suggestion.path;
      const select = document.createElement("select");
      select.id = label.htmlFor;
      select.dataset.path = suggestion.path;
      select.required = true;
      const placeholder = document.createElement("option");
      placeholder.value = "";
      placeholder.textContent = "请选择读取规则";
      select.append(placeholder);
      for (const rule of state.rules) {
        const option = document.createElement("option");
        option.value = rule.id;
        option.textContent = rule.name;
        select.append(option);
      }
      const remembered = state.rules.some(rule => rule.id === saved[suggestion.path]);
      select.value = remembered ? saved[suggestion.path] : suggestion.ruleId ?? "";
      const note = document.createElement("p");
      note.textContent = remembered ? "已恢复此文件上次使用的规则，可手动切换" : `${suggestion.note}（匹配 ${suggestion.matchedHeaders} 个日志头）`;
      select.addEventListener("change", () => {
        confirmFileRules.disabled = Array.from(fileAssignments.querySelectorAll("select")).some(item => !item.value);
      });
      row.append(label, select, note);
      fileAssignments.append(row);
    }
    confirmFileRules.disabled = !suggestions.length || Array.from(fileAssignments.querySelectorAll("select")).some(item => !item.value);
    fileFeedback.textContent = "请确认各文件的规则。没有合适的规则时，点击“管理规则”新增。";
  } catch (error) {
    if (request === fileRuleRequest) fileFeedback.textContent = `规则推荐失败：${error}`;
  }
}

fileRulesDialog.addEventListener("cancel", event => { if (importing) event.preventDefault(); });
fileRulesDialog.addEventListener("close", () => {
  fileRuleRequest++;
  if (!resumeFileRules) pendingPaths = null;
});
document.querySelector("#cancelFileRules").addEventListener("click", () => fileRulesDialog.close());
document.querySelector("#manageFileRules").addEventListener("click", () => {
  resumeFileRules = true;
  fileRulesDialog.close();
  void showReadingRules();
});
document.querySelector("#fileRulesForm").addEventListener("submit", async event => {
  event.preventDefault();
  if (importing || confirmFileRules.disabled) return;
  importing = true;
  fileRulesDialog.querySelectorAll("button, select").forEach(control => { control.disabled = true; });
  const request = ++loadRequest;
  try {
    const state = await readingRuleReady;
    const assignments = Array.from(fileAssignments.querySelectorAll("select"));
    const files = assignments.map(select => {
      const rule = state.rules.find(item => item.id === select.value);
      if (!rule) throw new Error("所选规则已不存在，请重新选择");
      return { path: select.dataset.path, rule };
    });
    fileFeedback.textContent = "正在分别读取并合并日志…";
    const result = await invoke("load_log_files", { files });
    if (request !== loadRequest) return;
    showLoadedData(result);
    fileRulesDialog.close();
    try {
      const bindings = JSON.parse(localStorage.getItem(FILE_RULES_KEY) ?? "{}");
      for (const file of files) bindings[file.path] = file.rule.id;
      localStorage.setItem(FILE_RULES_KEY, JSON.stringify(bindings));
    } catch (error) {
      setBusy(false, `日志已导入，但保存文件规则绑定失败：${error}`);
    }
  } catch (error) {
    fileFeedback.textContent = `导入失败：${error}`;
    setBusy(false, `导入失败：${error}`);
  } finally {
    importing = false;
    fileRulesDialog.querySelectorAll("button, select").forEach(control => { control.disabled = false; });
  }
});

function validColumnOrder(order) {
  return Array.isArray(order)
    && order.length === DEFAULT_COLUMN_ORDER.length
    && new Set(order).size === DEFAULT_COLUMN_ORDER.length
    && order.every(column => DEFAULT_COLUMN_ORDER.includes(column));
}

function applyColumnOrder(order) {
  const visible = order.filter(column => visibleColumns.has(column));
  elements.panel.style.setProperty("--column-layout", visible.map((column, index) => {
    return column === "message" || (!visible.includes("message") && index === visible.length - 1)
      ? "minmax(0, 1fr)" : `var(--width-${column})`;
  }).join(" "));
  order.forEach(column => {
    elements.panel.style.setProperty(`--position-${column}`, String(Math.max(1, visible.indexOf(column) + 1)));
    elements.panel.style.setProperty(`--display-${column}`, visibleColumns.has(column) ? "block" : "none");
    elements.header.append(elements.header.querySelector(`[data-column="${column}"]`));
  });
}

let visibleColumns = new Set(DEFAULT_COLUMN_ORDER);
try {
  const saved = localStorage.getItem(COLUMN_VISIBILITY_KEY);
  if (saved !== null) {
    const columns = JSON.parse(saved);
    if (!Array.isArray(columns) || columns.length === 0
      || new Set(columns).size !== columns.length
      || !columns.every(column => DEFAULT_COLUMN_ORDER.includes(column))) {
      throw new Error("Invalid visible columns");
    }
    visibleColumns = new Set(columns);
  }
} catch (error) {
  setBusy(false, `恢复显示列失败：${error}`);
}

let columnOrder = [...DEFAULT_COLUMN_ORDER];
try {
  const saved = localStorage.getItem(COLUMN_ORDER_KEY);
  if (saved !== null) {
    const order = JSON.parse(saved);
    if (!validColumnOrder(order)) throw new Error("Invalid column order");
    columnOrder = order;
  }
} catch (error) {
  setBusy(false, `恢复列顺序失败：${error}`);
}
applyColumnOrder(columnOrder);

function renderColumnSettings() {
  elements.columnSettingsOptions.replaceChildren();
  for (const column of columnOrder) {
    const label = document.createElement("label");
    const checkbox = document.createElement("input");
    checkbox.type = "checkbox";
    checkbox.checked = visibleColumns.has(column);
    checkbox.disabled = checkbox.checked && visibleColumns.size === 1;
    checkbox.addEventListener("change", () => {
      if (checkbox.checked) visibleColumns.add(column);
      else visibleColumns.delete(column);
      closeColumnFilters();
      applyColumnOrder(columnOrder);
      renderColumnSettings();
      try {
        localStorage.setItem(COLUMN_VISIBILITY_KEY, JSON.stringify([...visibleColumns]));
      } catch (error) {
        setBusy(false, `保存显示列失败：${error}`);
      }
    });
    const name = document.createElement("span");
    name.textContent = COLUMN_NAMES[column];
    label.append(checkbox, name);
    elements.columnSettingsOptions.append(label);
  }
}
elements.columnSettings.addEventListener("click", () => {
  const open = elements.columnSettingsOptions.hidden;
  closeColumnFilters();
  renderColumnSettings();
  elements.columnSettingsOptions.hidden = !open;
  elements.columnSettings.setAttribute("aria-expanded", String(open));
});

let draggedColumn = null;
elements.header.addEventListener("dragstart", event => {
  const column = event.target.closest("[data-column]");
  if (!column || event.target.closest(".column-filter-popover")) {
    event.preventDefault();
    return;
  }
  draggedColumn = column.dataset.column;
  closeColumnFilters();
  event.dataTransfer.effectAllowed = "move";
  event.dataTransfer.setData("text/plain", draggedColumn);
  column.classList.add("dragging");
});
elements.header.addEventListener("dragover", event => {
  const column = event.target.closest("[data-column]");
  if (!draggedColumn || !column) return;
  event.preventDefault();
  event.dataTransfer.dropEffect = "move";
  elements.header.querySelectorAll(".drop-target").forEach(item => item.classList.remove("drop-target"));
  column.classList.add("drop-target");
});
elements.header.addEventListener("drop", event => {
  const column = event.target.closest("[data-column]");
  if (!draggedColumn || !column) return;
  event.preventDefault();
  const next = columnOrder.filter(value => value !== draggedColumn);
  next.splice(columnOrder.indexOf(column.dataset.column), 0, draggedColumn);
  columnOrder = next;
  applyColumnOrder(columnOrder);
  try {
    localStorage.setItem(COLUMN_ORDER_KEY, JSON.stringify(columnOrder));
  } catch (error) {
    setBusy(false, `保存列顺序失败：${error}`);
  }
});
elements.header.addEventListener("dragend", () => {
  draggedColumn = null;
  elements.header.querySelectorAll(".dragging, .drop-target").forEach(item => {
    item.classList.remove("dragging", "drop-target");
  });
});

function renderSources() {
  selectedSources = new Set(sources.map((_, index) => index));
  renderColumnFilters();
  elements.source.title = sources.length
    ? `按文件筛选：${sources.join("  +  ")}`
    : "按文件筛选";
}

function renderColumnFilters() {
  for (const [trigger, values, names, selected, title] of [
    [elements.level, levels, levelNames, selectedLevels, "级别"],
    [elements.source, sources.map((_, index) => index), sources, selectedSources, "文件"],
  ]) {
    const panel = document.getElementById(trigger.getAttribute("aria-controls"));
    panel.replaceChildren();
    trigger.textContent = selected.size === values.length ? title : `${title} (${selected.size})`;
    trigger.classList.toggle("active", selected.size !== values.length);
    const addOption = (text, checked, onChange, indeterminate = false) => {
      const label = document.createElement("label");
      const checkbox = document.createElement("input");
      checkbox.type = "checkbox";
      checkbox.checked = checked;
      checkbox.indeterminate = indeterminate;
      checkbox.addEventListener("change", () => {
        onChange(checkbox.checked);
        renderColumnFilters();
        void applyFilters();
      });
      const span = document.createElement("span");
      span.textContent = text;
      label.title = text;
      label.append(checkbox, span);
      panel.append(label);
    };
    addOption("全部", values.length > 0 && selected.size === values.length, checked => {
      selected.clear();
      if (checked) values.forEach(value => selected.add(value));
    }, selected.size > 0 && selected.size < values.length);
    values.forEach((value, index) => addOption(names[index], selected.has(value), checked => {
      if (checked) selected.add(value);
      else selected.delete(value);
    }));
  }
}

function closeColumnFilters() {
  for (const trigger of [elements.level, elements.source]) {
    trigger.setAttribute("aria-expanded", "false");
    document.getElementById(trigger.getAttribute("aria-controls")).hidden = true;
  }
}

function resetColumnFilters() {
  selectedLevels = new Set(levels);
  selectedSources = new Set(sources.map((_, index) => index));
  renderColumnFilters();
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
  time.dataset.column = "timestamp";
  appendHighlighted(time, record.timestamp, record.timestampHighlightRanges);
  row.append(time);

  const level = document.createElement("span");
  level.dataset.column = "level";
  level.className = `level ${record.level.toLowerCase()}`;
  level.textContent = ({ INFO: "信息", WARN: "警告", ERROR: "错误", DEBUG: "调试", TRACE: "跟踪" })[record.level] ?? record.level;
  row.append(level);

  const source = document.createElement("span");
  source.dataset.column = "source";
  source.className = "source";
  source.title = record.source;
  source.textContent = record.source;
  row.append(source);

  const message = document.createElement("span");
  message.dataset.column = "message";
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
      keyword: elements.filterKeyword.value,
      levels: [...selectedLevels],
      sourceIndices: [...selectedSources],
      datasetVersion,
      revision: requestId,
    });
    if (requestId !== filterRequest) return;
    matchedCount = result.matchedCount;
    pageRequest++;
    pendingPageStart = null;
    pageStart = -1;
    pageRows = [];
    if (resetScroll) elements.viewport.scrollTop = 0;
    renderRows();
    setBusy(false, `筛选完成：${formatCount(matchedCount)} 条结果，耗时 ${result.filterMs} ms`);
  } catch (error) {
    setBusy(false, `筛选失败：${error}`);
  }
}

async function updateSearch() {
  if (!invoke || totalCount === 0) return;
  const requestId = ++searchRequest;
  try {
    await invoke("set_search", {
      keyword: elements.keyword.value,
      matchCase: searchOptions.matchCase,
      wholeWord: searchOptions.wholeWord,
      useRegex: searchOptions.useRegex,
      datasetVersion,
      revision: requestId,
    });
    if (requestId !== searchRequest) return;
    elements.keyword.removeAttribute("aria-invalid");
    pageRequest++;
    pendingPageStart = null;
    pageStart = -1;
    pageRows = [];
    renderRows();
  } catch (error) {
    if (requestId !== searchRequest) return;
    elements.keyword.setAttribute("aria-invalid", "true");
    setBusy(false, `搜索失败：${error}`);
  }
}

async function chooseFiles() {
  if (fileRulesDialog.open || readingDialog.open || importing) return;
  if (!invoke) {
    setBusy(false, "请在 Tauri 桌面窗口中选择本地日志文件");
    return;
  }
  try {
    const paths = await invoke("pick_log_files");
    if (!paths) return;
    await showFileRules(paths);
  } catch (error) {
    setBusy(false, `导入失败：${error}`);
  }
}

function showLoadedData(result) {
  filterRequest++;
  searchRequest++;
  pageRequest++;
  pendingPageStart = null;
  datasetVersion = result.datasetVersion;
  elements.keyword.value = "";
  elements.filterKeyword.value = "";
  elements.keyword.removeAttribute("aria-invalid");
  selectedLevels = new Set(levels);
  totalCount = result.totalCount;
  matchedCount = result.totalCount;
  sources = result.sources;
  pageStart = -1;
  pageRows = [];
  renderSources();
  elements.viewport.scrollTop = 0;
  elements.empty.textContent = totalCount ? "正在读取当前页…" : "没有可显示的有效日志行";
  elements.empty.hidden = totalCount !== 0;
  setBusy(false, `${result.algorithm} · 已合并 ${formatCount(totalCount)} 条${result.skippedLines ? `，跳过 ${formatCount(result.skippedLines)} 条无效行` : ""}`);
  renderRows();
}

renderColumnFilters();
for (const trigger of [elements.level, elements.source]) {
  trigger.addEventListener("click", () => {
    const open = trigger.getAttribute("aria-expanded") !== "true";
    closeColumnFilters();
    trigger.setAttribute("aria-expanded", String(open));
    document.getElementById(trigger.getAttribute("aria-controls")).hidden = !open;
  });
}
elements.filterKeyword.addEventListener("input", () => {
  clearTimeout(filterTimer);
  filterTimer = setTimeout(() => void applyFilters(), 250);
});
elements.keyword.addEventListener("input", () => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(() => void updateSearch(), 150);
});
const searchOptions = { matchCase: false, wholeWord: false, useRegex: false };
elements.searchOptions.addEventListener("click", event => {
  const button = event.target.closest("button[data-option]");
  if (!button) return;
  const option = button.dataset.option;
  searchOptions[option] = !searchOptions[option];
  button.setAttribute("aria-pressed", String(searchOptions[option]));
  void updateSearch();
});
elements.viewport.addEventListener("scroll", () => {
  if (scrollFrame) return;
  scrollFrame = requestAnimationFrame(() => {
    scrollFrame = 0;
    renderRows();
  });
});
window.addEventListener("resize", renderRows);

const titlebarMenus = Array.from(document.querySelectorAll(".titlebar-menu"));
const windowControls = document.querySelector(".window-controls");
if (!appWindow) windowControls.hidden = true;

function showSearch() {
  elements.searchField.hidden = false;
  elements.searchField.classList.remove("collapsed");
  elements.searchExpand.setAttribute("aria-expanded", "true");
  elements.filterKeyword.hidden = false;
  elements.keyword.focus();
}

function hideSearch() {
  elements.keyword.blur();
  elements.filterKeyword.blur();
  elements.searchField.hidden = true;
}

elements.searchExpand.addEventListener("click", () => {
  const expanded = elements.searchExpand.getAttribute("aria-expanded") === "true";
  elements.searchExpand.setAttribute("aria-expanded", String(!expanded));
  elements.searchField.classList.toggle("collapsed", expanded);
  elements.filterKeyword.hidden = expanded;
});
elements.searchClose.addEventListener("click", hideSearch);

function closeTitlebarMenus(except = null) {
  for (const menu of titlebarMenus) {
    if (menu === except) continue;
    menu.querySelector(".titlebar-menu-popover").hidden = true;
    menu.querySelector(".titlebar-menu-trigger").setAttribute("aria-expanded", "false");
  }
}

for (const menu of titlebarMenus) {
  const trigger = menu.querySelector(".titlebar-menu-trigger");
  const popover = menu.querySelector(".titlebar-menu-popover");
  trigger.addEventListener("click", () => {
    const shouldOpen = popover.hidden;
    closeTitlebarMenus();
    popover.hidden = !shouldOpen;
    trigger.setAttribute("aria-expanded", String(shouldOpen));
  });
  menu.querySelectorAll("[data-menu-action]").forEach(item => {
    item.addEventListener("click", () => {
      closeTitlebarMenus();
      switch (item.dataset.menuAction) {
        case "open_logs":
          void chooseFiles();
          break;
        case "focus_search":
          showSearch();
          break;
        case "clear_filters":
          elements.filterKeyword.value = "";
          resetColumnFilters();
          void applyFilters();
          break;
        case "reading_rules":
          void showReadingRules();
          break;
        case "quit":
          void appWindow?.close();
          break;
        case "about":
          window.alert("LogStryx\n跨平台日志检索与分析工作台");
          break;
      }
    });
  });
}

document.addEventListener("contextmenu", event => {
  event.preventDefault();
});

document.addEventListener("click", event => {
  if (!event.target.closest(".titlebar-menus")) closeTitlebarMenus();
  if (!event.target.closest(".column-filter")) closeColumnFilters();
  if (!event.target.closest(".column-settings")) {
    elements.columnSettingsOptions.hidden = true;
    elements.columnSettings.setAttribute("aria-expanded", "false");
  }
});

document.addEventListener("keydown", event => {
  if (readingDialog.open || fileRulesDialog.open) return;
  if (event.key === "Escape") {
    elements.columnSettingsOptions.hidden = true;
    elements.columnSettings.setAttribute("aria-expanded", "false");
    closeColumnFilters();
    closeTitlebarMenus();
    if (!elements.searchField.hidden) {
      hideSearch();
    }
  }
  if (!event.ctrlKey) return;
  switch (event.key.toLowerCase()) {
    case "o":
      event.preventDefault();
      void chooseFiles();
      break;
    case "f":
      event.preventDefault();
      showSearch();
      break;
    case "k":
      event.preventDefault();
      elements.filterKeyword.value = "";
      resetColumnFilters();
      void applyFilters();
      break;
  }
});

if (appWindow) {
  async function runWindowAction(action, label) {
    try {
      await action();
    } catch (error) {
      setBusy(false, `${label}失败：${error}`);
    }
  }

  document.querySelector(".window-control.minimize").addEventListener("click", () => {
    void runWindowAction(() => appWindow.minimize(), "最小化窗口");
  });
  document.querySelector(".window-control.maximize").addEventListener("click", () => {
    void runWindowAction(() => appWindow.toggleMaximize(), "切换窗口大小");
  });
  document.querySelector(".window-control.close").addEventListener("click", () => {
    void runWindowAction(() => appWindow.close(), "关闭窗口");
  });
}

renderRows();
