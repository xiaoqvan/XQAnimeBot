<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { api, type ProgressData } from "../api/client.ts";

const data = ref<ProgressData | null>(null);
const error = ref("");
const loading = ref(true);

let timer: ReturnType<typeof setInterval> | null = null;

async function refresh() {
  try {
    data.value = await api.getProgress();
    error.value = "";
  } catch (e) {
    error.value = (e as Error).message;
  } finally {
    loading.value = false;
  }
}

onMounted(() => {
  refresh();
  timer = setInterval(refresh, 5000); // 每 5 秒自动刷新
});

onUnmounted(() => {
  if (timer) clearInterval(timer);
});

async function cancel(title: string) {
  if (!confirm(`确定取消该任务？\n${title}`)) return;
  try {
    const res = await api.cancelProgress(title);
    alert(res.message);
    refresh();
  } catch (e) {
    alert((e as Error).message);
  }
}

async function cancelAll() {
  if (!confirm("确定取消全部活跃任务？")) return;
  try {
    await api.cancelAllProgress();
    refresh();
  } catch (e) {
    alert((e as Error).message);
  }
}

function elapsed(iso: string): string {
  const ms = Date.now() - new Date(iso).getTime();
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s} 秒`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} 分 ${s % 60} 秒`;
  return `${Math.floor(m / 60)} 时 ${m % 60} 分`;
}
</script>

<template>
  <div>
    <div class="head">
      <h2>⚙️ 处理进度</h2>
      <button class="btn btn-danger" @click="cancelAll">取消全部</button>
    </div>

    <div v-if="loading" class="card">加载中…</div>
    <div v-else-if="error" class="card error">{{ error }}</div>

    <template v-else-if="data">
      <div class="stat-grid">
        <div class="stat-card theme-blue">
          <div class="num">{{ data.activeCount }}</div>
          <div class="label">活跃任务</div>
        </div>
        <div class="stat-card theme-cyan">
          <div class="num">{{ data.queueSize }}</div>
          <div class="label">排队任务</div>
        </div>
        <div class="stat-card theme-violet">
          <div class="num">{{ Math.round(data.delay.intervalMs / 60000) }}</div>
          <div class="label">刷新间隔（分钟）</div>
        </div>
        <div class="stat-card theme-pink">
          <div class="num time">{{ new Date(data.delay.waitEnd).toLocaleTimeString() }}</div>
          <div class="label">下次刷新</div>
        </div>
      </div>

      <div v-if="data.items.length === 0" class="card empty">
        暂无活跃任务，等待下一轮 RSS 抓取…
      </div>

      <div v-else class="list">
        <div v-for="item in data.items" :key="item.title" class="card item">
          <div class="item-head">
            <div class="item-title">
              <span class="dot"></span>
              <strong>{{ item.animeName || "（解析中）" }}</strong>
            </div>
            <button class="btn btn-danger small" @click="cancel(item.title)">取消</button>
          </div>
          <p class="title" :title="item.title">🎬 {{ item.title }}</p>
          <div class="meta-grid">
            <span class="pill">📍 {{ item.stage }}</span>
            <span class="pill">⏱️ {{ elapsed(item.startTime) }}</span>
            <span v-if="item.qb" class="pill pill-qb">🧲 {{ item.qb.label }}</span>
          </div>
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.head {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.stat-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
  gap: 16px;
  margin-bottom: 22px;
}
.stat-card {
  border-radius: 16px;
  padding: 20px;
  text-align: center;
  border: 1px solid var(--border);
  transition: transform 0.2s;
}
.stat-card:hover {
  transform: translateY(-3px);
}
.theme-blue {
  background: var(--bg-blue);
  color: var(--blue);
}
.theme-cyan {
  background: var(--bg-cyan);
  color: var(--cyan);
}
.theme-violet {
  background: var(--bg-violet);
  color: var(--violet);
}
.theme-pink {
  background: var(--bg-pink);
  color: var(--pink);
}
.num {
  font-size: 30px;
  font-weight: 800;
}
.num.time {
  font-size: 20px;
  line-height: 40px;
}
.label {
  margin-top: 4px;
  font-size: 13px;
  opacity: 0.85;
}

.list {
  display: flex;
  flex-direction: column;
  gap: 14px;
}
.item-head {
  display: flex;
  justify-content: space-between;
  align-items: center;
}
.item-title {
  display: flex;
  align-items: center;
  gap: 8px;
}
.dot {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  background: var(--green);
  animation: pulse 1.6s ease-in-out infinite;
}
@keyframes pulse {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.35;
  }
}
.title {
  color: var(--text-soft);
  font-size: 13px;
  margin: 10px 0;
  word-break: break-all;
}
.meta-grid {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}
.pill {
  padding: 5px 12px;
  border-radius: 999px;
  font-size: 12px;
  background: #f3f4f6;
  border: 1px solid var(--border);
  color: var(--text);
}
.pill-qb {
  background: var(--bg-cyan);
  border-color: #c8eef5;
  color: var(--cyan);
}

.empty {
  color: var(--text-soft);
}

.btn.small {
  padding: 6px 12px;
  font-size: 12px;
  border-radius: 10px;
}
</style>
