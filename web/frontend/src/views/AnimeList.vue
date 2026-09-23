<script setup lang="ts">
import { onActivated, onMounted, ref } from "vue";
import { onBeforeRouteLeave, useRouter } from "vue-router";
import { api, readCache, type AnimeItem, type AnimePage, type SeasonItem } from "../api/client.ts";

// 滚动位置记忆：详情页返回后恢复到离开时的滚动位置
const SCROLL_KEY = "anime-list-scroll";
function saveScroll() {
    const el = document.querySelector<HTMLElement>(".content") ?? document.documentElement;
    sessionStorage.setItem(SCROLL_KEY, String(el.scrollTop ?? 0));
}
function restoreScroll() {
    const saved = Number(sessionStorage.getItem(SCROLL_KEY) ?? 0);
    const el = document.querySelector<HTMLElement>(".content") ?? document.documentElement;
    requestAnimationFrame(() => {
        el.scrollTop = saved;
    });
}

const router = useRouter();
const keyword = ref("");
const items = ref<AnimeItem[]>([]);
const error = ref("");
const loading = ref(false);
const searched = ref(false);

// 分页状态（浏览模式）
const page = ref(1);
const pageSize = ref(30);
const total = ref(0);
const totalPages = ref(0);

// 年季分类
const seasons = ref<SeasonItem[]>(readCache<{ items: SeasonItem[] }>("seasons")?.items ?? []);
const activeSeason = ref("all"); // "all" | SeasonKey | "unknown"
const seasonsLoading = ref(false);

// 添加番剧弹窗
const showAdd = ref(false);
const addSubjectId = ref("");
const adding = ref(false);
const addMsg = ref("");

async function loadSeasons() {
    seasonsLoading.value = true;
    try {
        const res = await api.listAnimeSeasons();
        seasons.value = res.items;
    } catch {
        seasons.value = [];
    } finally {
        seasonsLoading.value = false;
    }
}

async function loadPage(p = page.value) {
    // 缓存优先：刷新/切换时先用上次数据立即渲染，避免闪"加载中"
    const cacheKey = `anime:${activeSeason.value}:${p}:${pageSize.value}`;
    const cached = readCache<AnimePage>(cacheKey);
    if (cached) {
        items.value = cached.items;
        total.value = cached.total;
        totalPages.value = Math.max(1, Math.ceil(cached.total / cached.pageSize));
        page.value = cached.page;
        searched.value = false;
    }
    loading.value = !cached; // 有缓存则后台静默刷新，不闪"加载中"
    error.value = "";
    try {
        const res = await api.listAnime(p, pageSize.value, activeSeason.value);
        items.value = res.items;
        total.value = res.total;
        totalPages.value = Math.max(1, Math.ceil(res.total / res.pageSize));
        page.value = res.page;
        searched.value = false;
    } catch (e) {
        if (!cached) error.value = (e as Error).message;
    } finally {
        loading.value = false;
    }
}

function switchSeason(key: string) {
    activeSeason.value = key;
    page.value = 1;
    loadPage(1);
}

async function doSearch() {
    const q = keyword.value.trim();
    if (q.length < 2) {
        error.value = "关键词至少 2 个字符";
        return;
    }
    error.value = "";
    loading.value = true;
    try {
        const res = await api.searchAnime(q);
        items.value = res.items;
        searched.value = true;
    } catch (e) {
        error.value = (e as Error).message;
    } finally {
        loading.value = false;
    }
}

function clearSearch() {
    keyword.value = "";
    searched.value = false;
    page.value = 1;
    loadPage(1);
}

async function addAnime() {
    const subjectId = addSubjectId.value.trim();
    if (!subjectId) {
        addMsg.value = "请输入 Bangumi subject ID";
        return;
    }
    adding.value = true;
    addMsg.value = "";
    try {
        const res = await api.addAnime({ subjectId });
        addMsg.value = `✓ 已添加：${res.anime?.name_cn || res.anime?.name || ""}（ID ${res.id}）`;
        addSubjectId.value = "";
        loadPage(1);
    } catch (e) {
        addMsg.value = (e as Error).message;
    } finally {
        adding.value = false;
    }
}

function openDetail(id: number) {
    saveScroll();
    router.push(`/anime/${id}`);
}

// 首次进入：加载分类与第一页
onMounted(() => {
    loadSeasons();
    loadPage(1);
});

// keep-alive 重新激活（如从详情页返回）：若列表缓存已被清除（例如在详情页删除过番剧），
// 则按当前保存的页码/分类重新拉取；否则保留已有状态与滚动位置，实现"记忆翻页"。
onActivated(() => {
    const cacheKey = `anime:${activeSeason.value}:${page.value}:${pageSize.value}`;
    if (!readCache<AnimePage>(cacheKey)) {
        loadPage(page.value);
    }
    restoreScroll();
});

onBeforeRouteLeave(() => {
    saveScroll();
});
</script>

<template>
    <div>
        <div class="head">
            <h2>🎬 动漫库</h2>
            <button class="btn btn-primary" @click="showAdd = true">➕ 添加番剧</button>
        </div>

        <div class="searchbar card">
            <input v-model="keyword" placeholder="输入番剧名称或 ID 搜索" @keyup.enter="doSearch" />
            <button class="btn btn-primary" @click="doSearch">🔍 搜索</button>
            <button v-if="searched" class="btn btn-ghost" @click="clearSearch">↩ 返回列表</button>
        </div>

        <!-- 年季分类栏 -->
        <div v-if="!searched && seasonsLoading" class="card season-loading">加载分类…</div>
        <div v-else-if="!searched && seasons.length > 0" class="season-bar">
            <button class="season-chip" :class="{ active: activeSeason === 'all' }" @click="switchSeason('all')">
                全部 <span class="count">{{seasons.reduce((s, x) => s + x.count, 0)}}</span>
            </button>
            <button v-for="s in seasons" :key="s.key" class="season-chip" :class="{ active: activeSeason === s.key }"
                @click="switchSeason(s.key)">
                {{ s.label }} <span class="count">{{ s.count }}</span>
            </button>
        </div>

        <div v-if="error" class="card error">{{ error }}</div>

        <!-- 封面网格（浏览列表 或 搜索结果共用） -->
        <div v-if="!loading">
            <div class="result-hint">
                <template v-if="searched">
                    搜索 "{{ keyword }}" 共找到 <b>{{ items.length }}</b> 条
                </template>
                <template v-else>
                    共 <b>{{ total }}</b> 部番剧
                </template>
            </div>

            <div v-if="items.length === 0" class="card">暂无番剧</div>
            <div v-else class="grid">
                <div v-for="item in items" :key="item.id" class="anime-card" @click="openDetail(item.id)">
                    <div class="cover-wrap">
                        <img v-if="item.image" :src="item.image" alt="" class="cover" />
                        <div v-else class="cover placeholder">🎬</div>
                        <span v-if="item.r18" class="badge r18">R18</span>
                    </div>
                    <div class="body">
                        <div class="name">{{ item.name_cn || item.name }}</div>
                        <div class="meta">
                            <span class="chip">#{{ item.id }}</span>
                            <span class="chip chip-score">⭐ {{ item.score ?? "—" }}</span>
                            <span class="chip">{{ item.episode ?? "?" }} 集</span>
                        </div>
                    </div>
                </div>
            </div>

            <!-- 分页 -->
            <div v-if="!searched && totalPages > 1" class="pager">
                <button class="btn btn-ghost" :disabled="page <= 1" @click="loadPage(page - 1)">
                    ← 上一页
                </button>
                <span class="page-info">{{ page }} / {{ totalPages }}</span>
                <button class="btn btn-ghost" :disabled="page >= totalPages" @click="loadPage(page + 1)">
                    下一页 →
                </button>
            </div>
        </div>
        <div v-else class="card">加载中…</div>

        <!-- 添加番剧弹窗 -->
        <div v-if="showAdd" class="modal-mask" @click.self="showAdd = false">
            <div class="modal">
                <h3>➕ 添加番剧到动漫库</h3>
                <p class="muted">
                    通过 Bangumi subject ID 拉取信息并保存到动漫库（推荐）。也可在详情页用 ID 添加。
                </p>
                <input v-model="addSubjectId" placeholder="Bangumi subject ID（如 3382）" @keyup.enter="addAnime" />
                <div v-if="addMsg" class="add-msg">{{ addMsg }}</div>
                <div class="modal-actions">
                    <button class="btn btn-ghost" @click="showAdd = false">取消</button>
                    <button class="btn btn-primary" :disabled="adding" @click="addAnime">
                        {{ adding ? "添加中…" : "添加" }}
                    </button>
                </div>
            </div>
        </div>
    </div>
</template>

<style scoped>
.head {
    display: flex;
    align-items: center;
    justify-content: space-between;
}

.searchbar {
    display: flex;
    gap: 10px;
    margin-bottom: 20px;
    padding: 14px;
}

.searchbar input {
    flex: 1;
    padding: 11px 14px;
    border-radius: 12px;
    border: 1px solid var(--border);
    background: #fafbfc;
    color: var(--text);
    outline: none;
    transition: border-color 0.2s;
}

.searchbar input:focus {
    border-color: var(--blue);
    background: #fff;
}

/* 年季分类栏 */
.season-bar {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-bottom: 18px;
    padding: 12px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 14px;
}

.season-chip {
    padding: 7px 14px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: #fff;
    color: var(--text-soft);
    cursor: pointer;
    font-size: 13px;
    transition: all 0.15s;
}

.season-chip:hover {
    border-color: #cfd9f5;
    color: var(--blue);
}

.season-chip.active {
    background: var(--bg-blue);
    color: var(--blue);
    font-weight: 600;
    border-color: #c7d8f5;
}

.season-chip .count {
    font-size: 11px;
    opacity: 0.75;
    margin-left: 4px;
}

.season-loading {
    margin-bottom: 18px;
    color: var(--text-soft);
}

.result-hint {
    margin: 6px 0 14px;
    color: var(--text-soft);
}

.result-hint b {
    color: var(--blue);
}

.grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 18px;
}

.anime-card {
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 16px;
    overflow: hidden;
    cursor: pointer;
    transition: transform 0.2s, box-shadow 0.2s, border-color 0.2s;
}

.anime-card:hover {
    transform: translateY(-5px);
    border-color: #cfd9f5;
    box-shadow: 0 10px 28px rgba(16, 24, 40, 0.12);
}

.cover-wrap {
    position: relative;
    aspect-ratio: 3 / 4;
    background: linear-gradient(135deg, #ede9fe, #fdf2f8);
}

.cover {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
}

.cover.placeholder {
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 40px;
}

.badge {
    position: absolute;
    top: 10px;
    right: 10px;
    font-size: 11px;
    padding: 3px 8px;
    border-radius: 999px;
    font-weight: 700;
}

.r18 {
    background: #fdebed;
    color: var(--red);
}

.body {
    padding: 12px 13px 14px;
}

.delete-btn:hover {
    background: var(--red);
    color: #fff;
}

.name {
    font-weight: 700;
    font-size: 15px;
    line-height: 1.35;
    color: var(--text);
    display: -webkit-box;
    -webkit-line-clamp: 1;
    -webkit-box-orient: vertical;
    overflow: hidden;
}

.meta {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 10px;
}

.chip {
    font-size: 11px;
    padding: 3px 8px;
    border-radius: 999px;
    background: #f3f4f6;
    color: var(--text-soft);
}

.chip-score {
    background: var(--bg-amber);
    color: var(--amber);
}

.pager {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 16px;
    margin-top: 24px;
}

.page-info {
    color: var(--text-soft);
    font-weight: 600;
}

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

.modal input {
    width: 100%;
    padding: 11px 14px;
    border-radius: 12px;
    border: 1px solid var(--border);
    background: #fafbfc;
    color: var(--text);
    outline: none;
    margin-top: 8px;
}

.modal input:focus {
    border-color: var(--blue);
}

.muted {
    color: var(--text-soft);
    font-size: 13px;
}

.add-msg {
    margin-top: 10px;
    font-size: 13px;
    color: var(--green);
}

.add-msg:has(+ *) {
    color: var(--red);
}

.modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
    margin-top: 18px;
}
</style>
