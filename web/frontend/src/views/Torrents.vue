<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { api, readCache, type TorrentItem, type TorrentsResponse } from "../api/client.ts";

const items = ref<TorrentItem[]>(readCache<TorrentsResponse>("torrents")?.items ?? []);
const error = ref("");
const loading = ref(false);
const transfer = ref<{ dlSpeedLabel?: string; upSpeedLabel?: string }>(
    readCache<{ dlSpeedLabel?: string; upSpeedLabel?: string }>("transfer") ?? {}
);

let timer: ReturnType<typeof setInterval> | null = null;

async function refresh() {
    try {
        const [t, trans] = await Promise.all([
            api.getTorrents(),
            api.getTransfer().catch(() => null),
        ]);
        items.value = t.items;
        if (trans) transfer.value = trans;
        error.value = "";
    } catch (e) {
        error.value = (e as Error).message;
    } finally {
        loading.value = false;
    }
}

onMounted(() => {
    refresh();
    timer = setInterval(refresh, 5000);
});
onUnmounted(() => {
    if (timer) clearInterval(timer);
});

function pct(p: number): string {
    return `${(p * 100).toFixed(1)}%`;
}

function etaText(sec: number): string {
    if (!sec || sec < 0) return "—";
    const h = Math.floor(sec / 3600);
    const m = Math.floor((sec % 3600) / 60);
    if (h > 0) return `${h}时${m}分`;
    return `${m}分${sec % 60}秒`;
}

async function pause(t: TorrentItem) {
    await api.pauseTorrent(t.hash);
    refresh();
}
async function resume(t: TorrentItem) {
    await api.resumeTorrent(t.hash);
    refresh();
}
async function remove(t: TorrentItem) {
    if (!confirm(`删除种子（含数据）？\n${t.name}`)) return;
    await api.deleteTorrent(t.hash, true);
    refresh();
}
async function removeNoFiles(t: TorrentItem) {
    if (!confirm(`仅删除种子（保留文件）？\n${t.name}`)) return;
    await api.deleteTorrent(t.hash, false);
    refresh();
}
</script>

<template>
    <div>
        <h2>🧲 BT 下载</h2>

        <div v-if="loading" class="card">加载中…</div>
        <div v-else-if="error" class="card error">{{ error }}</div>

        <template v-else>
            <div class="transfer card">
                <span>⬇️ 下载 <b>{{ transfer.dlSpeedLabel }}</b></span>
                <span>⬆️ 上传 <b>{{ transfer.upSpeedLabel }}</b></span>
                <span>种子数 <b>{{ items.length }}</b></span>
            </div>

            <div v-if="items.length === 0" class="card">暂无 BT 种子</div>
            <div v-else class="list">
                <div v-for="t in items" :key="t.hash" class="card item">
                    <div class="item-head">
                        <div class="item-title">
                            <span class="dot" :class="t.state"></span>
                            <div>
                                <div class="name">{{ t.name }}</div>
                                <div class="sub">
                                    {{ t.stateLabel }} ｜ {{ pct(t.progress) }} ｜ {{ t.sizeLabel }} ｜ 做种
                                    {{ t.num_seeds }} / 下载 {{ t.num_leechs }}
                                </div>
                            </div>
                        </div>
                        <div class="actions">
                            <button v-if="['pausedDL', 'pausedUP', 'stoppedDL', 'stoppedUP'].includes(t.state)"
                                class="btn btn-ghost small" @click="resume(t)">
                                ▶ 恢复
                            </button>
                            <button v-else class="btn btn-ghost small" @click="pause(t)">
                                ⏸ 暂停
                            </button>
                            <button class="btn btn-ghost small" @click="removeNoFiles(t)">🗑 删种子</button>
                            <button class="btn btn-danger small" @click="remove(t)">🗑 删除</button>
                        </div>
                    </div>
                    <div class="progress-track">
                        <div class="progress-fill" :style="{ width: pct(t.progress) }"></div>
                    </div>
                    <div class="meta-row">
                        <span>⬇️ {{ t.dlspeedLabel }}</span>
                        <span>⬆️ {{ t.upspeedLabel }}</span>
                        <span>⏳ {{ etaText(t.eta) }}</span>
                        <span>比率 {{ t.ratio.toFixed(2) }}</span>
                    </div>
                </div>
            </div>
        </template>
    </div>
</template>

<style scoped>
.transfer {
    display: flex;
    gap: 28px;
    flex-wrap: wrap;
    margin-bottom: 18px;
    color: var(--text-soft);
}

.transfer b {
    color: var(--blue);
}

.list {
    display: flex;
    flex-direction: column;
    gap: 14px;
}

.item-head {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    align-items: flex-start;
}

.item-title {
    display: flex;
    gap: 10px;
    min-width: 0;
}

.dot {
    width: 10px;
    height: 10px;
    min-width: 10px;
    border-radius: 50%;
    margin-top: 6px;
    background: #9ca3af;
}

.dot.downloading,
.dot.forcedDL,
.dot.metaDL {
    background: var(--blue);
}

.dot.uploading,
.dot.forcedUP {
    background: var(--green);
}

.dot.stoppedUP {
    background: var(--green);
}

.dot.error,
.dot.missingFiles {
    background: var(--red);
}

.dot.pausedDL,
.dot.pausedUP {
    background: var(--amber);
}

.name {
    font-weight: 700;
    word-break: break-all;
}

.sub {
    color: var(--text-soft);
    font-size: 12px;
    margin-top: 4px;
}

.actions {
    display: flex;
    gap: 6px;
    flex-shrink: 0;
}

.btn.small {
    padding: 6px 10px;
    font-size: 12px;
}

.progress-track {
    height: 8px;
    border-radius: 999px;
    background: #eef1f5;
    margin: 12px 0 10px;
    overflow: hidden;
}

.progress-fill {
    height: 100%;
    border-radius: 999px;
    background: linear-gradient(90deg, #4a7bd8, #3aa8b8);
    transition: width 0.4s;
}

.meta-row {
    display: flex;
    gap: 18px;
    flex-wrap: wrap;
    font-size: 12px;
    color: var(--text-soft);
}
</style>
