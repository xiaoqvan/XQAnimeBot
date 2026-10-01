/**
 * 父进程看门狗：父进程（BOT）被 OOM/强杀后清理绑定的子进程树。
 *
 * 用法: node processWatchdog.mjs <parentPid> <pidFile> <isWin:0|1>
 */
import { readFileSync, existsSync, unlinkSync } from "node:fs";
import { spawn } from "node:child_process";

const parentPid = Number(process.argv[2] ?? 0);
const pidFile = process.argv[3] ?? "";
const isWin = process.argv[4] === "1";

if (!parentPid || !pidFile) {
  process.exit(1);
}

function parentAlive() {
  try {
    process.kill(parentPid, 0);
    return true;
  } catch {
    return false;
  }
}

function killTree(pid) {
  if (!pid || pid === process.pid || pid === parentPid) return;
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
    // 进程可能已退出
  }
}

function cleanupAll() {
  try {
    if (existsSync(pidFile)) {
      const raw = readFileSync(pidFile, "utf8");
      for (const token of raw.split(/[\s,]+/)) {
        const pid = Number(token);
        if (Number.isFinite(pid) && pid > 0) killTree(pid);
      }
      unlinkSync(pidFile);
    }
  } catch {
    // ignore
  }
}

// 父进程仍在时只做存在性探测；一旦消失立即清扫
const timer = setInterval(() => {
  if (!parentAlive()) {
    clearInterval(timer);
    cleanupAll();
    process.exit(0);
  }
}, 1500);

// 看门狗自身被终止时也尽量清扫
for (const sig of ["SIGINT", "SIGTERM", "SIGHUP"]) {
  process.on(sig, () => {
    clearInterval(timer);
    cleanupAll();
    process.exit(0);
  });
}

process.on("exit", () => {
  clearInterval(timer);
});
