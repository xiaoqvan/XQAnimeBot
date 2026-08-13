<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { api, type ReviewItem } from "../api/client.ts";

const router = useRouter();
const items = ref<ReviewItem[]>([]);
const error = ref("");
const loading = ref(true);
const flow = ref<"pre_post" | "pre_review">("pre_post");
const filter = ref("pending");
const total = ref(0);
const page = ref(1);
const pageSize = ref(30);
const totalPages = ref(1);
const acting = ref<number | null>(null);

async function refresh(p = 1) {
  loading.value = true;
  error.value = "";
  try {
    const res = await api.listReviews(flow.value, filter.value, p, pageSize.value);
    items.value = res.items;
    total.value = res.total ?? res.items.length;
    totalPages.value = Math.max(1, Math.ceil((res.total ?? res.items.length) / (res.pageSize ?? pageSize.value)));
    page.value = res.page ?? p;
  } catch (e) {
    error.value = (e as Error).message;
  } finally {
    loading.value = false;
  }
}

function switchFlow(f: "pre_post" | "pre_review") {
  flow.value = f;
  filter.value = "pending";
  page.value = 1;
  refresh(1);
}

function switchFilter(s: string) {
  filter.value = s;
  page.value = 1;
  refresh(1);
}

async function approve(r: ReviewItem) {
  // 先审核后发流程不支持 approve（缓存转正由 bot 处理），给出提示
  if (flow.value === "pre_review") {
    alert("先审核后发流程由 Bot 回调处理，请在 Telegram 中确认/纠正。");
    return;
  }
  acting.value = r.id;
  try {
    await api.approveReview(r.id);
    refresh(page.value);
  } catch (e) {
    alert((e as Error).message);
  } finally {
    acting.value = null;
  }
}

async function reject(r: ReviewItem, remove: boolean) {
  const msg = remove ? "标记并移除该待确认记录？" : "标记该记录为已拒绝？";
  if (!confirm(`确认操作（${r.animeName}）？\n${msg}`)) return;
  acting.value = r.id;
  try {
    await api.rejectReview(r.id, remove);
    refresh(page.value);
  } catch (e) {
    alert((e as Error).message);
  } finally {
    acting.value = null;
  }
}

onMounted(() => refresh(1));
</script>

<template>
  <div>
    <div class="head">
      <h2>📝 待确认番剧</h2>
      <div class="tabs">
        <button
          class="tab flow"
          :class="{ active: flow === 'pre_post' }"
          @click="switchFlow('pre_post')"
        >
          先发后审
        </button>
        <button
          class="tab flow"
          :class="{ active: flow === 'pre_review' }"
          @click="switchFlow('pre_review')"
        >
          先审核后发
        </button>
      </div>
    </div>

    <div class="status-tabs">
      <button
        v-for="s in ['pending', 'approved', 'rejected']"
        :key="s"
        class="tab"
        :class="{ active: filter === s }"
        @click="switchFilter(s)"
      >
        {{ { pending: "待确认", approved: "已确认", rejected: "已拒绝" }[s] }}
      </button>
    </div>

    <div v-if="loading" class="card">加载中…</div>
    <div v-else-if="error" class="card error">{{ error }}</div>

    <div v-else-if="items.length === 0" class="card">
      暂无{{ filter === "pending" ? "待确认" : filter === "approved" ? "已确认" : "已拒绝" }}
      {{ flow === "pre_post" ? "（先发后审）" : "（先审核后发）" }}记录
    </div>

    <template v-else>
      <div class="result-hint">共 <b>{{ total }}</b> 条</div>
      <div class="list">
        <div v-for="r in items" :key="r.id" class="card item">
          <div class="item-head">
            <div>
              <div class="name">
                {{ r.animeName }}
                <span class="badge" :class="r.status">
                  {{ { pending: "待确认", approved: "已确认", rejected: "已拒绝" }[r.status] }}
                </span>
                <span class="flow-badge" :class="flow">
                  {{ flow === "pre_post" ? "先发后审" : "先审核后发" }}
                </span>
              </div>
              <div class="sub">
                记录 ID {{ r.id }} ｜ 番剧 ID {{ r.animeId }}
                <template v-if="flow === 'pre_post'">｜ 第 {{ r.episodeSort }} 集</template>
                <template v-if="r.createdAt && r.createdAt !== undefined">｜
                  {{ new Date(r.createdAt).toLocaleString() }}
                </template>
              </div>
            </div>
            <div v-if="r.status === 'pending' && flow === 'pre_post'" class="actions">
              <button class="btn btn-primary small" :disabled="acting === r.id" @click="approve(r)">
                ✓ 确认
              </button>
              <button class="btn btn-danger small" :disabled="acting === r.id" @click="reject(r, true)">
                ✗ 拒绝并移除
              </button>
              <button class="btn btn-ghost small" :disabled="acting === r.id" @click="reject(r, false)">
                标记拒绝
              </button>
            </div>
            <div v-else-if="r.status === 'pending' && flow === 'pre_review'" class="actions">
              <span class="hint-inline">请在 Telegram Bot 中确认/纠正（先审核后发）</span>
            </div>
          </div>
          <p class="title">🎬 {{ r.title }}</p>
          <div v-if="r.matchReason" class="reason">
            🤖 AI 匹配（置信度
            {{ r.matchConfidence !== undefined ? (r.matchConfidence * 100).toFixed(0) : "—" }}
            %）：{{ r.matchReason }}
          </div>
          <button class="link-btn" @click="router.push(`/anime/${r.animeId}`)">
            查看番剧 →
          </button>
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
.tab.flow {
  padding: 8px 18px;
  font-size: 14px;
}
.tab.flow.active {
  background: var(--bg-violet);
  color: var(--violet);
  border-color: #dcd2f7;
}
.status-tabs {
  display: flex;
  gap: 6px;
  margin: 14px 0 18px;
}
.result-hint {
  margin: 6px 0 14px;
  color: var(--text-soft);
}
.result-hint b {
  color: var(--blue);
}
.flow-badge {
  margin-left: 8px;
  font-size: 11px;
  padding: 2px 8px;
  border-radius: 999px;
  font-weight: 700;
}
.flow-badge.pre_post {
  background: var(--bg-blue);
  color: var(--blue);
}
.flow-badge.pre_review {
  background: var(--bg-violet);
  color: var(--violet);
}
.hint-inline {
  font-size: 12px;
  color: var(--text-mute);
  padding: 6px 4px;
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
.name {
  font-weight: 700;
  font-size: 15px;
}
.badge {
  margin-left: 8px;
  font-size: 11px;
  padding: 2px 8px;
  border-radius: 999px;
  font-weight: 700;
}
.badge.pending {
  background: var(--bg-amber);
  color: var(--amber);
}
.badge.approved {
  background: var(--bg-green);
  color: var(--green);
}
.badge.rejected {
  background: #fdebed;
  color: var(--red);
}
.sub {
  color: var(--text-soft);
  font-size: 12px;
  margin-top: 4px;
}
.actions {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
}
.btn.small {
  padding: 6px 10px;
  font-size: 12px;
}
.title {
  color: var(--text-soft);
  font-size: 13px;
  margin: 12px 0 8px;
  word-break: break-all;
}
.reason {
  background: var(--bg-violet);
  color: var(--violet);
  border-radius: 10px;
  padding: 10px 12px;
  font-size: 13px;
  line-height: 1.6;
}
.link-btn {
  margin-top: 12px;
  background: none;
  border: none;
  color: var(--blue);
  cursor: pointer;
  font-size: 13px;
  padding: 0;
}
.link-btn:hover {
  text-decoration: underline;
}
</style>
