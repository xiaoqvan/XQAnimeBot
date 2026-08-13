<script setup lang="ts">
import { onMounted, ref } from "vue";
import { api, type AiCallItem } from "../api/client.ts";

const items = ref<AiCallItem[]>([]);
const error = ref("");
const loading = ref(true);
const page = ref(1);
const totalPages = ref(1);
const total = ref(0);
const scene = ref("");
const expanded = ref<number | null>(null);

async function refresh(p = page.value) {
    loading.value = true;
    error.value = "";
    try {
        const res = await api.listAiCalls(p, 20, scene.value || undefined);
        items.value = res.items;
        total.value = res.total;
        totalPages.value = Math.max(1, Math.ceil(res.total / res.pageSize));
        page.value = res.page;
    } catch (e) {
        error.value = (e as Error).message;
    } finally {
        loading.value = false;
    }
}

function switchScene(s: string) {
    scene.value = s;
    refresh(1);
}

function toggle(i: number) {
    expanded.value = expanded.value === i ? null : i;
}

function dur(ms?: number): string {
    if (!ms) return "—";
    return ms < 1000 ? `${ms}ms` : `${(ms / 1000).toFixed(2)}s`;
}

onMounted(() => refresh(1));
</script>

<template>
    <div>
        <div class="head">
            <h2>🤖 AI 调用记录</h2>
            <div class="tabs">
                <button class="tab" :class="{ active: scene === '' }" @click="switchScene('')">全部</button>
                <button class="tab" :class="{ active: scene === 'bangumi_match' }"
                    @click="switchScene('bangumi_match')">番剧匹配</button>
                <button class="tab" :class="{ active: scene === 'episode_match' }"
                    @click="switchScene('episode_match')">集数匹配</button>
                <button class="tab" :class="{ active: scene === 'episode_extract' }"
                    @click="switchScene('episode_extract')">集数提取</button>
            </div>
        </div>

        <div v-if="loading" class="card">加载中…</div>
        <div v-else-if="error" class="card error">{{ error }}</div>

        <div v-else-if="items.length === 0" class="card">暂无 AI 调用记录</div>

        <template v-else>
            <div class="result-hint">共 <b>{{ total }}</b> 条记录</div>
            <div class="list">
                <div v-for="(c, idx) in items" :key="idx" class="card item" @click="toggle(idx)">
                    <div class="item-head">
                        <div class="item-title">
                            <span class="dot" :class="c.success ? 'ok' : 'fail'"></span>
                            <div>
                                <div class="name">
                                    {{ c.sceneLabel }}
                                    <span class="chip" :class="c.success ? 'chip-ok' : 'chip-fail'">
                                        {{ c.success ? "成功" : "失败" }}
                                    </span>
                                </div>
                                <div class="sub">
                                    {{ new Date(c.createdAt).toLocaleString() }} ｜ 耗时 {{ dur(c.durationMs) }}
                                </div>
                            </div>
                        </div>
                        <span class="chevron">{{ expanded === idx ? "▲" : "▼" }}</span>
                    </div>

                    <template v-if="expanded === idx">
                        <div class="block">
                            <div class="block-label">输入</div>
                            <pre class="content">{{ c.input }}</pre>
                        </div>
                        <div v-if="c.output" class="block">
                            <div class="block-label">输出</div>
                            <pre class="content">{{ c.output }}</pre>
                        </div>
                        <div v-if="c.meta" class="block">
                            <div class="block-label">附加信息</div>
                            <pre class="content">{{ c.meta }}</pre>
                        </div>
                    </template>
                </div>
            </div>

            <div v-if="totalPages > 1" class="pager">
                <button class="btn btn-ghost" :disabled="page <= 1" @click="refresh(page - 1)">← 上一页</button>
                <span class="page-info">{{ page }} / {{ totalPages }}</span>
                <button class="btn btn-ghost" :disabled="page >= totalPages" @click="refresh(page + 1)">下一页 →</button>
            </div>
        </template>
    </div>
</template>

<style scoped>
.head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 12px;
}

.tabs {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
}

.tab {
    padding: 7px 14px;
    border-radius: 10px;
    border: 1px solid var(--border);
    background: #fff;
    color: var(--text-soft);
    cursor: pointer;
    font-size: 13px;
}

.tab.active {
    background: var(--bg-blue);
    color: var(--blue);
    font-weight: 600;
    border-color: #c7d8f5;
}

.result-hint {
    margin: 6px 0 14px;
    color: var(--text-soft);
}

.result-hint b {
    color: var(--blue);
}

.list {
    display: flex;
    flex-direction: column;
    gap: 14px;
}

.item {
    cursor: pointer;
}

.item-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
}

.item-title {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    min-width: 0;
}

.dot {
    width: 10px;
    height: 10px;
    min-width: 10px;
    border-radius: 50%;
    margin-top: 6px;
}

.dot.ok {
    background: var(--green);
}

.dot.fail {
    background: var(--red);
}

.name {
    font-weight: 700;
}

.sub {
    color: var(--text-soft);
    font-size: 12px;
    margin-top: 4px;
}

.chip {
    margin-left: 6px;
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 999px;
    font-weight: 700;
}

.chip-ok {
    background: var(--bg-green);
    color: var(--green);
}

.chip-fail {
    background: #fdebed;
    color: var(--red);
}

.chevron {
    color: var(--text-mute);
}

.block {
    margin-top: 12px;
}

.block-label {
    font-size: 12px;
    color: var(--text-mute);
    margin-bottom: 4px;
}

.content {
    white-space: pre-wrap;
    word-break: break-word;
    background: #f6f8fb;
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 10px 12px;
    font-size: 12px;
    font-family: monospace;
    color: var(--text);
    max-height: 240px;
    overflow: auto;
    margin: 0;
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
</style>
