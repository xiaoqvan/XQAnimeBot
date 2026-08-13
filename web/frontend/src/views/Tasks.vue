<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { api, readCache, type BtTask, type BtTaskType } from "../api/client.ts";

const tasks = ref<BtTask[]>(readCache<{ items: BtTask[] }>("tasks")?.items ?? []);
const error = ref("");
const loading = ref(false);

// 创建任务表单
const taskType = ref<BtTaskType>("addanime");
const taskEpid = ref("");
const taskUrl = ref("");
const creating = ref(false);
const createMsg = ref("");

let timer: ReturnType<typeof setInterval> | null = null;

const statusMap: Record<string, { label: string; cls: string }> = {
    queued: { label: "排队中", cls: "queued" },
    running: { label: "进行中", cls: "running" },
    done: { label: "已完成", cls: "done" },
    failed: { label: "失败", cls: "failed" },
    canceled: { label: "已取消", cls: "canceled" },
};

async function refresh(showLoading = false) {
    // 后台静默刷新：列表已有/有缓存时不闪"加载中"，只在真正首次无数据时显示
    if (showLoading) loading.value = true;
    error.value = "";
    try {
        const res = await api.listTasks();
        tasks.value = res.items;
    } catch (e) {
        error.value = (e as Error).message;
    } finally {
        loading.value = false;
    }
}

async function createTask() {
    const epid = taskEpid.value.trim();
    const url = taskUrl.value.trim();
    if (!epid || !url) {
        createMsg.value = "请填写集数 ID 和 BT 链接";
        return;
    }
    creating.value = true;
    createMsg.value = "";
    try {
        const res = await api.createTask(taskType.value, epid, url);
        createMsg.value = `✓ 已创建任务 #${res.id}`;
        taskEpid.value = "";
        taskUrl.value = "";
        refresh();
    } catch (e) {
        createMsg.value = (e as Error).message;
    } finally {
        creating.value = false;
    }
}

async function cancel(t: BtTask) {
    if (!confirm(`取消任务 #${t.id}「${t.animeName || t.title || t.type}」？`)) return;
    try {
        await api.cancelTask(t.id);
        refresh();
    } catch (e) {
        alert((e as Error).message);
    }
}

async function clear() {
    if (!confirm("清理所有已完成/失败/已取消的任务？")) return;
    try {
        await api.clearTasks();
        refresh();
    } catch (e) {
        alert((e as Error).message);
    }
}

function fmt(iso?: string): string {
    if (!iso) return "—";
    return new Date(iso).toLocaleTimeString();
}

onMounted(() => {
    // 首次：无缓存（tasks 为空）时显示加载中；有缓存则静默刷新
    refresh(tasks.value.length === 0);
    timer = setInterval(() => refresh(), 3000); // 后台静默轮询，不闪"加载中"
});
onUnmounted(() => {
    if (timer) clearInterval(timer);
});
</script>

<template>
    <div>
        <div class="head">
            <h2>⚙️ BT 任务</h2>
            <button class="btn btn-ghost" @click="clear">🗑 清理已完成</button>
        </div>
        <p class="desc">
            在 Web 中触发 addanime（为已有番剧添加 BT 信息）或 addnewanime（添加新番剧并关联 BT
            信息），任务在后台队列执行并实时显示进度。
        </p>

        <!-- 创建表单 -->
        <div class="create card">
            <div class="type-tabs">
                <button class="tab" :class="{ active: taskType === 'addanime' }" @click="taskType = 'addanime'">
                    addanime（已有番剧）
                </button>
                <button class="tab" :class="{ active: taskType === 'addnewanime' }" @click="taskType = 'addnewanime'">
                    addnewanime（新番剧）
                </button>
            </div>
            <div class="fields">
                <input v-model="taskEpid" placeholder="Bangumi 集数 ID（episode id）" />
                <input v-model="taskUrl" placeholder="BT 链接（bangumi.moe / dmhy）" style="flex: 1.6" />
                <button class="btn btn-primary" :disabled="creating" @click="createTask">
                    {{ creating ? "创建中…" : "创建任务" }}
                </button>
            </div>
            <div v-if="createMsg" class="create-msg">{{ createMsg }}</div>
        </div>

        <div v-if="loading" class="card">加载中…</div>
        <div v-else-if="error" class="card error">{{ error }}</div>

        <div v-else-if="tasks.length === 0" class="card">暂无任务</div>

        <div v-else class="list">
            <div v-for="t in tasks" :key="t.id" class="card item">
                <div class="item-head">
                    <div>
                        <span class="badge" :class="statusMap[t.status]?.cls">
                            {{ statusMap[t.status]?.label || t.status }}
                        </span>
                        <span class="type-badge" :class="t.type">#{{ t.id }} · {{ t.type }}</span>
                        <span class="name">{{ t.animeName || t.title || "解析中…" }}</span>
                    </div>
                    <div class="actions">
                        <span v-if="t.stage" class="stage">{{ t.stage }}</span>
                        <button v-if="t.status === 'running' || t.status === 'queued'" class="btn btn-danger small"
                            @click="cancel(t)">
                            取消
                        </button>
                    </div>
                </div>
                <p class="title">{{ t.title }}</p>
                <div class="meta">
                    创建 {{ fmt(t.createdAt) }} ｜ 开始 {{ fmt(t.startTime) }} ｜ 更新 {{ fmt(t.updatedAt) }}
                </div>
                <div v-if="t.error" class="err">{{ t.error }}</div>
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

.desc {
    color: var(--text-soft);
    font-size: 13px;
    margin-top: -8px;
    margin-bottom: 16px;
}

.create {
    margin-bottom: 18px;
}

.type-tabs {
    display: flex;
    gap: 6px;
    margin-bottom: 12px;
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

.fields {
    display: flex;
    gap: 10px;
}

.fields input {
    flex: 1;
    padding: 10px 12px;
    border-radius: 10px;
    border: 1px solid var(--border);
    background: #fafbfc;
    color: var(--text);
    outline: none;
}

.fields input:focus {
    border-color: var(--blue);
    background: #fff;
}

.create-msg {
    margin-top: 10px;
    font-size: 13px;
    color: var(--green);
}

.list {
    display: flex;
    flex-direction: column;
    gap: 12px;
}

.item-head {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    align-items: flex-start;
}

.badge {
    font-size: 11px;
    padding: 3px 9px;
    border-radius: 999px;
    font-weight: 700;
}

.badge.queued {
    background: var(--bg-amber);
    color: var(--amber);
}

.badge.running {
    background: var(--bg-blue);
    color: var(--blue);
}

.badge.done {
    background: var(--bg-green);
    color: var(--green);
}

.badge.failed {
    background: #fdebed;
    color: var(--red);
}

.badge.canceled {
    background: #f1f2f6;
    color: var(--text-mute);
}

.type-badge {
    margin-left: 8px;
    font-size: 11px;
    padding: 3px 9px;
    border-radius: 999px;
    background: var(--bg-violet);
    color: var(--violet);
}

.name {
    font-weight: 700;
    margin-left: 8px;
}

.actions {
    display: flex;
    align-items: center;
    gap: 10px;
}

.stage {
    font-size: 12px;
    color: var(--text-soft);
}

.btn.small {
    padding: 6px 10px;
    font-size: 12px;
}

.title {
    color: var(--text-soft);
    font-size: 13px;
    margin: 10px 0 4px;
    word-break: break-all;
}

.meta {
    font-size: 12px;
    color: var(--text-mute);
}

.err {
    margin-top: 8px;
    font-size: 12px;
    color: var(--red);
    background: #fdf1f3;
    padding: 8px 10px;
    border-radius: 8px;
}
</style>
