<script setup lang="ts">
import { onMounted, ref } from "vue";
import { api, type Stats } from "../api/client.ts";

const stats = ref<Stats | null>(null);
const error = ref("");
const loading = ref(true);

onMounted(async () => {
  try {
    stats.value = await api.stats();
  } catch (e) {
    error.value = (e as Error).message;
  } finally {
    loading.value = false;
  }
});
</script>

<template>
  <div>
    <h2>📊 概览</h2>

    <div v-if="loading" class="card">加载中…</div>
    <div v-else-if="error" class="card error">{{ error }}</div>

    <template v-else-if="stats">
      <div class="grid">
        <div class="stat-card theme-violet">
          <div class="stat-ico">🎬</div>
          <div class="num">{{ stats.animeCount }}</div>
          <div class="label">番剧总数</div>
        </div>
        <div class="stat-card theme-blue">
          <div class="stat-ico">🗂️</div>
          <div class="num">{{ stats.episodeCount }}</div>
          <div class="label">章节元数据</div>
        </div>
        <div class="stat-card theme-cyan">
          <div class="stat-ico">🧲</div>
          <div class="num">{{ stats.torrentCount }}</div>
          <div class="label">BT 种子记录</div>
        </div>
        <div class="stat-card theme-pink">
          <div class="stat-ico">⚙️</div>
          <div class="num">{{ stats.processing.active }}</div>
          <div class="label">活跃处理任务</div>
        </div>
      </div>

      <div class="card status-card">
        <h3>⚡ 处理状态</h3>
        <div class="pills">
          <span class="pill pill-blue">活跃 {{ stats.processing.active }}</span>
          <span class="pill pill-cyan">排队 {{ stats.processing.queued }}</span>
          <span class="pill pill-violet">缓存番剧 {{ stats.cacheAnimeCount }}</span>
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(170px, 1fr));
  gap: 18px;
  margin-bottom: 22px;
}

/* 淡彩纯色统计卡片，白色主题点缀 */
.stat-card {
  border-radius: 18px;
  padding: 24px 20px;
  text-align: center;
  border: 1px solid var(--border);
  transition: transform 0.2s, box-shadow 0.2s;
}
.stat-card:hover {
  transform: translateY(-3px);
  box-shadow: 0 8px 22px rgba(16, 24, 40, 0.1);
}
.theme-violet {
  background: var(--bg-violet);
  color: var(--violet);
}
.theme-blue {
  background: var(--bg-blue);
  color: var(--blue);
}
.theme-cyan {
  background: var(--bg-cyan);
  color: var(--cyan);
}
.theme-pink {
  background: var(--bg-pink);
  color: var(--pink);
}

.stat-ico {
  font-size: 26px;
  margin-bottom: 8px;
  filter: none;
}
.num {
  font-size: 34px;
  font-weight: 800;
  line-height: 1.1;
}
.label {
  margin-top: 6px;
  font-size: 13px;
  opacity: 0.85;
}

/* 处理状态卡片内的淡彩药丸标签 */
.pills {
  display: flex;
  flex-wrap: wrap;
  gap: 12px;
}
.pill {
  padding: 8px 16px;
  border-radius: 999px;
  font-size: 13px;
  font-weight: 600;
}
.pill-blue {
  background: var(--bg-blue);
  color: var(--blue);
}
.pill-cyan {
  background: var(--bg-cyan);
  color: var(--cyan);
}
.pill-violet {
  background: var(--bg-violet);
  color: var(--violet);
}
</style>
