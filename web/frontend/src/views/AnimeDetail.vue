<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { api, readCache, type AnimeItem, type AnimeDetail } from "../api/client.ts";

const route = useRoute();
const router = useRouter();
const anime = ref<AnimeItem | null>(
    readCache<AnimeDetail>(`anime-detail:${route.params.id}`)?.anime ?? null
);
const error = ref("");
const loading = ref(!anime.value);

// 删除：二级确认（先打开确认弹窗，再点"确认删除"才执行）
const deleting = ref(false);
const confirmOpen = ref(false);

/** 该番剧各字幕组：resources 按字幕组分组，key 即字幕组名 */
const subtitleGroups = computed<string[]>(() => {
    const res = (anime.value as { resources?: Record<string, unknown[]> } | null)?.resources;
    if (!res || typeof res !== "object") return [];
    return Object.keys(res).filter((k) => k && k !== "unknown");
});
const hasSubtitleGroups = computed(() => subtitleGroups.value.length > 0);

async function confirmDelete() {
    if (!anime.value) return;
    deleting.value = true;
    try {
        await api.deleteAnime(anime.value.id);
        alert(`已删除「${anime.value.name_cn || anime.value.name}」`);
        router.push("/anime");
    } catch (e) {
        alert((e as Error).message);
    } finally {
        deleting.value = false;
        confirmOpen.value = false;
    }
}

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
        <div class="toolbar">
            <button class="btn btn-ghost back" @click="router.back()">← 返回</button>
            <button v-if="anime" class="btn btn-danger" :disabled="deleting" @click="confirmOpen = true">
                🗑 删除
            </button>
        </div>

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

            <!-- 字幕组提示：该番剧由这些字幕组制作过资源 -->
            <div v-if="hasSubtitleGroups" class="card">
                <h3>🎙️ 字幕组</h3>
                <p class="muted subt-hint">以下字幕组为本番剧提供过资源，可按字幕组筛选观看。</p>
                <div class="tags">
                    <span v-for="g in subtitleGroups" :key="g" class="tag fan-sub">{{ g }}</span>
                </div>
            </div>

            <div v-if="anime.tags?.length" class="card">
                <h3>🏷️ 标签</h3>
                <div class="tags">
                    <span v-for="t in anime.tags" :key="t" class="tag">#{{ t }}</span>
                </div>
            </div>
        </div>

        <!-- 二级确认弹窗：点"删除"后弹出，需再次点"确认删除"才真正删除 -->
        <div v-if="confirmOpen && anime" class="modal-mask" @click.self="!deleting && (confirmOpen = false)">
            <div class="modal">
                <h3>🗑 确认删除番剧</h3>
                <p class="muted">
                    确定要删除 <b>{{ anime.name_cn || anime.name }}</b>（ID {{ anime.id }}）吗？
                    此操作将同时删除其<span class="danger-text">章节、资源与待审核记录</span>，且<strong>无法撤销</strong>。
                </p>
                <p class="muted">（BT 去重记录会保留）</p>
                <div class="modal-actions">
                    <button class="btn btn-ghost" :disabled="deleting" @click="confirmOpen = false">
                        取消
                    </button>
                    <button class="btn btn-danger" :disabled="deleting" @click="confirmDelete">
                        {{ deleting ? "删除中…" : "确认删除" }}
                    </button>
                </div>
            </div>
        </div>
    </div>
</template>

<style scoped>
.toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 16px;
}

.back {
    padding: 10px 14px;
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

/* 字幕组标签 */
.fan-sub {
    color: var(--cyan);
    background: var(--bg-cyan);
    border-color: #cbeef2;
    font-weight: 600;
}

.subt-hint {
    margin: -6px 0 12px;
}

.danger-text {
    color: var(--red);
    font-weight: 600;
}

/* 二级确认弹窗 */
.modal-mask {
    position: fixed;
    inset: 0;
    background: rgba(15, 23, 42, 0.4);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
    padding: 20px;
}

.modal {
    background: var(--card);
    border-radius: 16px;
    padding: 24px;
    width: 100%;
    max-width: 420px;
    box-shadow: 0 20px 50px rgba(0, 0, 0, 0.25);
}

.modal h3 {
    margin-top: 0;
}

.modal .muted {
    line-height: 1.7;
}

.modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
    margin-top: 18px;
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
