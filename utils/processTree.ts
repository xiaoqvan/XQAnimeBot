import { spawn, type ChildProcess, type SpawnOptions } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { randomBytes } from "node:crypto";

const isWin = process.platform === "win32";
const selfDir = dirname(fileURLToPath(import.meta.url));
const watchdogScript = join(selfDir, "processWatchdog.mjs");

/** 本进程唯一 pid 注册表文件 */
const pidFile = join(tmpdir(), `xqbot-children-${process.pid}-${randomBytes(4).toString("hex")}.pid`);

const tracked = new Map<number, ChildProcess>();
let watchdog: ChildProcess | null = null;
let hooksInstalled = false;
let shuttingDown = false;

function ensurePidDir() {
  // tmpdir 一定存在；仅为兼容异常环境
  try {
    mkdirSync(dirname(pidFile), { recursive: true });
  } catch {
    /* ignore */
  }
}

function rewritePidFile() {
  ensurePidDir();
  const pids = [...tracked.keys()];
  if (pids.length === 0) {
    try {
      rmSync(pidFile, { force: true });
    } catch {
      /* ignore */
    }
    return;
  }
  writeFileSync(pidFile, pids.join("\n"), "utf8");
}

function appendPid(pid: number) {
  ensurePidDir();
  try {
    appendFileSync(pidFile, `${pid}\n`, "utf8");
  } catch {
    rewritePidFile();
  }
}

/** 杀掉整棵进程树（含孙进程） */
export function killProcessTree(pid: number | undefined): void {
  if (!pid) return;
  try {
    if (isWin) {
      spawn("taskkill", ["/PID", String(pid), "/T", "/F"], {
        stdio: "ignore",
        windowsHide: true,
      });
    } else {
      try {
        process.kill(-pid, "SIGKILL");
      } catch {
        process.kill(pid, "SIGKILL");
      }
    }
  } catch {
    /* 已退出 */
  }
}

/** 清理本模块登记过的全部子进程树 */
export function killAllTracked(): void {
  for (const [pid, child] of tracked) {
    if (!child.killed) killProcessTree(pid);
  }
  tracked.clear();
  rewritePidFile();
}

function installExitHooks() {
  if (hooksInstalled) return;
  hooksInstalled = true;

  const onExit = () => {
    if (shuttingDown) return;
    shuttingDown = true;
    killAllTracked();
    stopWatchdog();
  };

  process.on("exit", onExit);
  for (const sig of ["SIGINT", "SIGTERM", "SIGHUP"] as const) {
    process.on(sig, () => {
      onExit();
      process.exit(sig === "SIGINT" ? 130 : 143);
    });
  }
  process.on("uncaughtException", (err) => {
    onExit();
    console.error("[processTree] uncaughtException, children reaped:", err);
    process.exit(1);
  });
  process.on("unhandledRejection", () => {
    // 不在这里杀进程：让上层决定；仅保证 exit 时有 hook
  });
}

/**
 * 启动父进程看门狗：BOT 被 OOM/强杀时收割子进程。
 * 看门狗本身 detached，不随 BOT 退出。
 */
function ensureWatchdog() {
  if (watchdog && watchdog.exitCode === null && !watchdog.killed) return;

  ensurePidDir();
  // 先写出当前已知 pid，避免看门狗启动瞬间读到空文件后父进程再补
  rewritePidFile();

  watchdog = spawn(
    process.execPath,
    [watchdogScript, String(process.pid), pidFile, isWin ? "1" : "0"],
    {
      detached: true,
      stdio: "ignore",
      windowsHide: true,
    }
  );
  watchdog.unref();
}

function stopWatchdog() {
  if (!watchdog) return;
  try {
    if (watchdog.pid) {
      if (isWin) {
        spawn("taskkill", ["/PID", String(watchdog.pid), "/T", "/F"], {
          stdio: "ignore",
          windowsHide: true,
        });
      } else {
        watchdog.kill("SIGTERM");
      }
    }
  } catch {
    /* ignore */
  }
  watchdog = null;
}

export interface BoundSpawnOptions extends SpawnOptions {
  /**
   * true 时子进程进入独立进程组（Unix）/ 仍纳入树绑定（Windows），
   * 便于 killProcessTree 整树收割。默认 true。
   */
  ownProcessGroup?: boolean;
}

/**
 * 启动子进程并绑定到进程树生命周期。
 *
 * - 父进程正常退出 / 信号 / 未捕获异常 → 立刻 taskkill / kill 码整树
 * - 爥进程被 OOM / 强杀 → 看门狗按 pid 注册表收割
 *
 * @param cmd 可执行文件
 * @param args 参数
 * @param options spawn 选项
 */
export function spawnBound(
  cmd: string,
  args: string[],
  options: BoundSpawnOptions = {}
): ChildProcess {
  installExitHooks();
  ensureWatchdog();

  const { ownProcessGroup = true, ...rest } = options;
  const child = spawn(cmd, args, {
    ...rest,
    // Unix: 独立进程组，kill(-pid) 可一次收掉 ffmpeg 及其子进程
    // Windows: 不用 detached（会让子进程脱离控制台且更难管），走 taskkill /T
    detached: ownProcessGroup && !isWin,
    windowsHide: true,
  });

  const pid = child.pid;
  if (pid && pid !== process.pid) {
    tracked.set(pid, child);
    appendPid(pid);
  }

  const untrack = () => {
    if (pid) {
      tracked.delete(pid);
      rewritePidFile();
    }
    if (tracked.size === 0) {
      stopWatchdog();
    }
  };

  child.once("error", untrack);
  child.once("close", untrack);

  return child;
}

/**
 * 类似 run() 的 Promise 封装，带进程树绑定。
 */
export function runBound(
  cmd: string,
  args: string[],
  opts: {
    onStderr?: (chunk: string) => void;
    onStdout?: (chunk: string) => void;
    captureStdout?: boolean;
  } = {}
): Promise<{ stdout: string; stderr: string }> {
  return new Promise((resolve, reject) => {
    const stdio: SpawnOptions["stdio"] = opts.captureStdout
      ? ["ignore", "pipe", "pipe"]
      : ["ignore", opts.onStdout ? "pipe" : "ignore", "pipe"];

    const p = spawnBound(cmd, args, { stdio });
    let stdout = "";
    let stderr = "";

    p.stdout?.on("data", (d) => {
      const s = d.toString();
      stdout += s;
      opts.onStdout?.(s);
    });
    p.stderr?.on("data", (d) => {
      const s = d.toString();
      stderr += s;
      opts.onStderr?.(s);
    });

    p.on("error", (err) =>
      reject(new Error(`${cmd} 启动失败: ${err.message}`))
    );
    p.on("close", (code) => {
      if (code === 0) {
        resolve({ stdout, stderr });
      } else {
        const detail = stderr.trim() ? `\nstderr: ${stderr.trim()}` : "";
        reject(new Error(`${cmd} exited with ${code}${detail}`));
      }
    });
  });
}

/** 启动时预热：保证首个 ffmpeg 之前看门狗已就绪（可选调用） */
export function warmupProcessTreeBinding(): void {
  installExitHooks();
  ensureWatchdog();
}

/** 仅测试用：当前登记的子进程 pid */
export function listTrackedPids(): number[] {
  return [...tracked.keys()];
}

// 防止模块被重复加载时 pid 文件路径漂移；路径含 pid+随机，始终唯一
if (!existsSync(dirname(pidFile))) {
  try {
    mkdirSync(dirname(pidFile), { recursive: true });
  } catch {
    /* ignore */
  }
}
