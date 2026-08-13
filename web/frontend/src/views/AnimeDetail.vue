<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { api, type AnimeItem } from "../api/client.ts";

const route = useRoute();
const router = useRouter();
const anime = ref<AnimeItem | null>(null);
const error = ref("");
const loading = ref(true);

onMounted(async () => {
  try {
    const res = await api.getAnime(String(route.params.id));
    anime.value = res.anime;
  } catch (e) {
    error.value = (e as Error).message;
  } finally {
    loading.value = false;
  }
});
</script>

<template>
  <div>
    <button class="btn btn-ghost back" @click="router.back()">← 返回</button>

    <div v-if="loading" class="card">加载中…</div>
    <div v-else-if="error" class="card error">{{ error }}</div>

    <div v-else-if="anime" class="detail">
      <div class="card head">
        <div class="cover-wrap">
          <img v-if="anime.image" :src="anime.image" alt="" class="cover" />
          <div v-else class="cover placeholder">🎬</div>
        </div>
        <div class="head-info">
          <h2>
            {{ anime.name_cn || anime.name }}
            <span v-if="anime.r18" class="badge r18">R18</span>
            <span v-if="anime.airingDay" class="badge day">周 {{ anime.airingDay }}</span>
          </h2>
          <p class="muted original">原名：{{ anime.name }}</p>
          <div class="meta-row">
            <span class="pill">⭐ {{ anime.score ?? "—" }}</span>
            <span class="pill">🎞️ {{ anime.episode ?? "?" }} 集</span>
            <span class="pill">🆔 {{ anime.id }}</span>
          </div>
          <p v-if="anime.summary" class="summary">{{ anime.summary }}</p>
        </div>
      </div>

      <div v-if="anime.tags?.length" class="card">
        <h3>🏷️ 标签</h3>
        <div class="tags">
          <span v-for="t in anime.tags" :key="t" class="tag">#{{ t }}</span>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.back {
  margin-bottom: 16px;
}

.head {
  display: flex;
  gap: 24px;
}
.cover-wrap {
  flex-shrink: 0;
  width: 180px;
  border-radius: 14px;
  overflow: hidden;
  background: linear-gradient(135deg, #ede9fe, #fdf2f8);
  border: 1px solid var(--border);
}
.cover {
  width: 100%;
  height: 250px;
  object-fit: cover;
  display: block;
}
.cover.placeholder {
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 56px;
}
.head-info {
  flex: 1;
}
.head-info h2 {
  margin-top: 0;
  line-height: 1.3;
}
.badge {
  display: inline-block;
  font-size: 12px;
  padding: 3px 10px;
  border-radius: 999px;
  font-weight: 700;
  margin-left: 6px;
  vertical-align: middle;
}
.r18 {
  background: #fdebed;
  color: var(--red);
}
.day {
  background: var(--bg-green);
  color: var(--green);
}
.muted {
  color: var(--text-soft);
  margin: 8px 0;
}
.meta-row {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
  margin: 12px 0;
}
.pill {
  padding: 5px 13px;
  border-radius: 999px;
  font-size: 13px;
  font-weight: 600;
  background: #f3f4f6;
  border: 1px solid var(--border);
  color: var(--text);
}
.summary {
  line-height: 1.8;
  color: var(--text-soft);
}

.tags {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}
.tag {
  padding: 6px 13px;
  border-radius: 999px;
  font-size: 13px;
  color: var(--violet);
  background: var(--bg-violet);
  border: 1px solid #e4dcfb;
}

.detail {
  display: flex;
  flex-direction: column;
  gap: 18px;
}

@media (max-width: 640px) {
  .head {
    flex-direction: column;
  }
}
</style>
