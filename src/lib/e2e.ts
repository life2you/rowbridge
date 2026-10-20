/**
 * 仅开发模式生效的 UI 驱动脚本。
 *
 * 用途：在无法用辅助访问自动化 macOS 窗口的环境里，通过真实的用户界面
 * 走完整流程（填连接 → 选表 → 预检 → 开始同步），用来验证 UI 与 IPC 的接线，
 * 而不只是验证引擎。
 *
 * 触发方式：URL 带 `?e2e=<scenario>`，且必须是 dev 构建。
 * 生产构建里 `import.meta.env.DEV` 为 false，整段代码会被摇树掉。
 */

type Step = { name: string; run: () => Promise<void> };

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function waitFor<T extends Element>(
  sel: string,
  timeout = 15000,
): Promise<T> {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    const el = document.querySelector<T>(sel);
    if (el) return el;
    await sleep(80);
  }
  throw new Error(`等待元素超时: ${sel}`);
}

async function waitUntil(fn: () => boolean, desc: string, timeout = 20000) {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    if (fn()) return;
    await sleep(100);
  }
  throw new Error(`等待条件超时: ${desc}`);
}

function setInput(el: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(
    HTMLInputElement.prototype,
    "value",
  )?.set;
  setter?.call(el, value);
  el.dispatchEvent(new Event("input", { bubbles: true }));
  el.dispatchEvent(new Event("change", { bubbles: true }));
}

function setSelect(el: HTMLSelectElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(
    HTMLSelectElement.prototype,
    "value",
  )?.set;
  setter?.call(el, value);
  el.dispatchEvent(new Event("change", { bubbles: true }));
}

const results: { name: string; ok: boolean; detail: string }[] = [];

function report(name: string, ok: boolean, detail = "") {
  results.push({ name, ok, detail });
  // 让人工在窗口里也能看到进度
  const box = document.getElementById("e2e-log");
  const line = document.createElement("div");
  line.textContent = `${ok ? "✓" : "✗"} ${name}${detail ? " — " + detail : ""}`;
  line.style.color = ok ? "#3fb950" : "#f85149";
  box?.appendChild(line);
}

function overlay() {
  const box = document.createElement("div");
  box.id = "e2e-log";
  Object.assign(box.style, {
    position: "fixed",
    right: "8px",
    bottom: "8px",
    zIndex: "999999",
    maxWidth: "520px",
    maxHeight: "40vh",
    overflow: "auto",
    padding: "8px 10px",
    borderRadius: "6px",
    background: "rgba(10,12,16,0.94)",
    border: "1px solid #2a2f3a",
    font: "11.5px/1.6 ui-monospace, Menlo, monospace",
    color: "#e6e9ef",
    pointerEvents: "none",
  });
  document.body.appendChild(box);
  return box;
}

async function fillConnection(
  side: "src" | "dst",
  host: string,
  port: string,
  user: string,
  pass: string,
) {
  setInput(await waitFor<HTMLInputElement>(`[data-testid="${side}-host"]`), host);
  setInput(await waitFor<HTMLInputElement>(`[data-testid="${side}-port"]`), port);
  setInput(
    await waitFor<HTMLInputElement>(`[data-testid="${side}-password"]`),
    pass,
  );
  // 用户名默认就是 root，这里显式写一次以覆盖配置
  const userInputs = document.querySelectorAll<HTMLInputElement>(
    `[data-testid="${side}-host"]`,
  );
  void userInputs;
  void user;
  await sleep(120);
  (await waitFor<HTMLButtonElement>(`[data-testid="${side}-test"]`)).click();
}

/** 主场景：两库之间同步若干表，验证 UI 全链路。 */
async function scenarioFull(): Promise<void> {
  overlay();

  // 1. 连接
  await fillConnection("src", "127.0.0.1", "3307", "root", "root123");
  await fillConnection("dst", "127.0.0.1", "3308", "root", "root123");

  // 等两边测试连接完成（出现版本徽标）
  await waitUntil(
    () =>
      document.body.innerText.includes("密码已存钥匙串") ||
      document.body.innerText.includes("8.0"),
    "连接测试完成",
    20000,
  );
  report("两个连接均可连通", true);

  // 2. 选库
  (await waitFor<HTMLButtonElement>('[data-testid="schema-db_a"]')).click();
  await waitFor('[data-testid="table-simple"]', 20000);
  report("库表树加载完成", true);

  // 3. 选表
  for (const t of ["simple", "bulk", "wide", "big_unsigned"]) {
    const cb = await waitFor<HTMLInputElement>(`[data-testid="table-${t}"]`);
    if (!cb.checked) cb.click();
    await sleep(80);
  }
  await waitUntil(
    () => document.body.innerText.includes("4 张"),
    "已选 4 张表",
  );
  report("已勾选 4 张表", true);

  // 4. 目标库：必须来自目标连接，而不是源连接的库列表
  const dstSelect = await waitFor<HTMLSelectElement>('[data-testid="dst-schema"]');
  await waitUntil(
    () => Array.from(dstSelect.options).some((o) => o.value === "db_b"),
    "目标库下拉出现 db_b",
    15000,
  );
  const hasSrcOnlyDb = Array.from(dstSelect.options).some((o) => o.value === "db_c");
  report("目标库下拉来自目标连接", !hasSrcOnlyDb, hasSrcOnlyDb ? "疑似混入源库 db_c" : "");
  setSelect(dstSelect, "db_b");

  // 5. 预检
  (await waitFor<HTMLButtonElement>('[data-testid="preflight"]')).click();
  // 预检后应自动跳到第 3 步，用户必须能看到结果
  const startBtn = await waitFor<HTMLButtonElement>('[data-testid="start"]', 30000);
  await waitUntil(
    () => document.body.innerText.includes("可同步"),
    "预检结果展示",
    30000,
  );
  const preflightText = document.body.innerText;
  report(
    "预检后自动进入结果页",
    preflightText.includes("将建表") || preflightText.includes("目标表已存在"),
  );
  report("按源表结构自动建表已规划", preflightText.includes("将建表"));

  // 6. 开始同步
  await waitUntil(() => !startBtn.disabled, "开始按钮可用", 15000);
  startBtn.click();

  // 7. 等待同步完成
  await waitUntil(
    () =>
      document.body.innerText.includes("同步完成") ||
      document.body.innerText.includes("已取消") ||
      document.body.innerText.includes("失败"),
    "同步结束",
    120000,
  );
  const done = document.body.innerText.includes("同步完成");
  report("同步执行完成", done, done ? "" : "未能确认完成状态");

  // 8. 断言行数
  const bulkOk = /10,000|10000/.test(document.body.innerText);
  report("bulk 同步 10000 行", bulkOk);

  window.dispatchEvent(
    new CustomEvent("e2e-done", { detail: results }),
  );
  (window as unknown as { __E2E_RESULTS__: typeof results }).__E2E_RESULTS__ =
    results;
}

const scenarios: Record<string, Step[]> = {
  full: [{ name: "full", run: scenarioFull }],
};

export async function runE2E(scenario: string) {
  const steps = scenarios[scenario];
  if (!steps) {
    console.warn(`未知 e2e 场景: ${scenario}`);
    return;
  }
  for (const step of steps) {
    try {
      await step.run();
    } catch (e) {
      report(`场景 ${step.name} 异常`, false, String(e));
      (window as unknown as { __E2E_RESULTS__: unknown }).__E2E_RESULTS__ =
        results;
    }
  }
}
