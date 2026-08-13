<script setup lang="ts">
import { ref, onMounted, watch } from "vue";
import { useRoute } from "vue-router";
import { api, getToken, setToken, getApiBase, setApiBase, configureConnection } from "./api/client.ts";

// 登录状态：未配置或密钥无效时显示登录页
// 已保存过连接则初始即就绪（刷新整页直接进主界面，不再闪登录页/加载中），后台再校验密钥
const ready = ref(!!getToken() && !!getApiBase());
const checking = ref(false);
const authError = ref("");
const connecting = ref(false);

// 移动端抽屉菜单
const menuOpen = ref(false);
const route = useRoute();
watch(
    () => route.fullPath,
    () => {
        menuOpen.value = false; // 路由切换后关闭抽屉
    }
);

// URL 参数自动注入（按钮跳转带 ?api=xxx&key=xxx）
function autoInjectFromUrl(): boolean {
    const q = new URLSearchParams(window.location.search);
    // 兼容参数可能位于 hash 内（如 /#/...?api=&key=）
    const hashQ = new URLSearchParams(
        (window.location.hash.split("?")[1] ?? "").split("#")[0]
    );
    const apiParam = q.get("api") || hashQ.get("api");
    const keyParam = q.get("key") || hashQ.get("key");
    if (apiParam && keyParam) {
        configureConnection(decodeURIComponent(apiParam), decodeURIComponent(keyParam));
        // 清除 URL 参数，避免刷新后重复注入
        const clean = window.location.origin + window.location.pathname + window.location.hash;
        window.history.replaceState(null, "", clean);
        return true;
    }
    return false;
}

async function verify(): Promise<boolean> {
    try {
        await api.me();
        return true;
    } catch {
        return false;
    }
}

async function init() {
    checking.value = true;
    authError.value = "";
    try {
        autoInjectFromUrl();
        // 无配置 → 未就绪
        if (!getToken() || !getApiBase()) {
            ready.value = false;
            return;
        }
        // 有配置 → 验证密钥
        const ok = await verify();
        ready.value = ok;
        if (!ok) {
            authError.value = "密钥无效或已过期，请重新连接（可回复 Bot /web 获取新密钥）。";
        }
    } finally {
        checking.value = false;
    }
}

async function doConnect() {
    connecting.value = true;
    authError.value = "";
    try {
        setToken(keyInput.value.trim());
        setApiBase(apiInput.value);
        const ok = await verify();
        if (ok) {
            ready.value = true;
        } else {
            authError.value = "无法连接或密钥无效，请检查地址与密钥。";
        }
    } catch (e) {
        authError.value = (e as Error).message;
    } finally {
        connecting.value = false;
    }
}

function logout() {
    localStorage.removeItem("xq_anime_web_token");
    localStorage.removeItem("xq_anime_web_api");
    ready.value = false;
    authError.value = "";
}

const apiInput = ref(getApiBase() === "/api" ? "" : getApiBase());
const keyInput = ref("");

onMounted(() => {
    init();
});
</script>

<template>
    <!-- 登录页 -->
    <div v-if="!ready" class="login-page">
        <div class="login-card">
            <h1 class="logo">✨ XQ 动漫</h1>
            <p class="login-desc">请输入后端 API 地址与访问密钥。密钥可在 Bot 中回复 <code>/web</code> 获取（主人/管理员），有效期 24 小时。</p>
            <div v-if="checking" class="login-status">正在检查…</div>
            <template v-else>
                <input v-model="apiInput" placeholder="API 地址，如 http://host:3780/api" />
                <input v-model="keyInput" type="password" placeholder="访问密钥" @keyup.enter="doConnect" />
                <div v-if="authError" class="login-error">{{ authError }}</div>
                <button class="btn btn-primary connect-btn" :disabled="connecting" @click="doConnect">
                    {{ connecting ? "连接中…" : "连接" }}
                </button>
            </template>
        </div>
    </div>

    <!-- 主界面 -->
    <div v-else class="layout">
        <!-- 移动端顶栏 -->
        <header class="mobile-topbar">
            <button class="hamburger" @click="menuOpen = true" aria-label="打开菜单">☰</button>
            <h1 class="logo">✨ XQ 动漫</h1>
        </header>

        <!-- 移动端遮罩：点击关闭抽屉 -->
        <div v-if="menuOpen" class="scrim" @click="menuOpen = false"></div>

        <aside class="sidebar" :class="{ open: menuOpen }">
            <button class="sidebar-close" @click="menuOpen = false" aria-label="关闭菜单">✕</button>
            <h1 class="logo">✨ XQ 动漫</h1>
            <nav>
                <RouterLink to="/">
                    <span class="nav-ico">📊</span> 概览
                </RouterLink>
                <RouterLink to="/anime">
                    <span class="nav-ico">🎬</span> 动漫库
                </RouterLink>
                <RouterLink to="/torrents">
                    <span class="nav-ico">🧲</span> BT 下载
                </RouterLink>
                <RouterLink to="/tasks">
                    <span class="nav-ico">⚙️</span> BT 任务
                </RouterLink>
                <RouterLink to="/reviews">
                    <span class="nav-ico">📝</span> 待确认番剧
                </RouterLink>
                <RouterLink to="/ai">
                    <span class="nav-ico">🤖</span> AI 记录
                </RouterLink>
                <RouterLink to="/progress">
                    <span class="nav-ico">⚙️</span> 处理进度
                </RouterLink>
            </nav>

            <div class="sidebar-bottom">
                <button class="logout-btn" @click="logout">🔓 断开连接</button>
            </div>
        </aside>

        <main class="content">
            <RouterView />
        </main>
    </div>
</template>

<style>
:root {
    /* 浅色主题淡彩点缀色板 */
    --violet: #7c5cd6;
    --blue: #4a7bd8;
    --cyan: #3aa8b8;
    --pink: #d66a9a;
    --amber: #c9903c;
    --green: #3f9d74;
    --red: #d94a5f;

    /* 淡彩纯色背景（用于卡片点缀） */
    --bg-violet: #f1ecff;
    --bg-blue: #e8f1ff;
    --bg-cyan: #e6f8fb;
    --bg-pink: #fdeef6;
    --bg-green: #e9f7f0;
    --bg-amber: #fbf3e4;

    /* 文字 */
    --text: #1f2937;
    --text-soft: #4b5563;
    --text-mute: #9ca3af;

    /* 边框与背景 */
    --border: #e5e7eb;
    --bg: #f5f6fa;
    --card: #ffffff;
}

* {
    box-sizing: border-box;
}

html,
body {
    margin: 0;
    min-height: 100%;
}

body {
    font-family: "Segoe UI", system-ui, -apple-system, "PingFang SC", sans-serif;
    color: var(--text);
    background: var(--bg);
    background-image:
        radial-gradient(800px 400px at 85% -5%, rgba(124, 92, 214, 0.08), transparent 60%),
        radial-gradient(700px 400px at -5% 10%, rgba(74, 123, 216, 0.07), transparent 55%);
    background-attachment: fixed;
    overflow-x: hidden;
}

.layout {
    display: flex;
    min-height: 100vh;
}

/* ===== 移动端顶栏 / 抽屉 ===== */
.mobile-topbar {
    display: none; /* 默认（桌面）隐藏 */
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    z-index: 30;
    height: 56px;
    align-items: center;
    gap: 12px;
    padding: 0 16px;
    background: var(--card);
    border-bottom: 1px solid var(--border);
}

.hamburger {
    width: 40px;
    height: 40px;
    border: none;
    border-radius: 10px;
    background: var(--bg-blue);
    color: var(--blue);
    font-size: 20px;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
}

.sidebar-close {
    display: none;
    position: absolute;
    top: 14px;
    right: 14px;
    width: 34px;
    height: 34px;
    border: none;
    border-radius: 10px;
    background: #f3f4f6;
    color: var(--text-soft);
    font-size: 15px;
    cursor: pointer;
}

.scrim {
    display: none;
    position: fixed;
    inset: 0;
    z-index: 19;
    background: rgba(17, 24, 39, 0.45);
}

@media (max-width: 820px) {
    /* 顶栏显示、内容区避开顶栏 */
    .mobile-topbar {
        display: flex;
    }
    .mobile-topbar .logo {
        font-size: 18px;
    }

    .layout {
        display: block;
        padding-top: 56px;
    }

    /* 侧边栏 → 抽屉，默认滑出屏幕外 */
    .sidebar {
        position: fixed;
        z-index: 20;
        left: 0;
        top: 0;
        width: 80vw;
        max-width: 300px;
        height: 100vh;
        transform: translateX(-100%);
        transition: transform 0.28s ease;
        box-shadow: 0 0 40px rgba(16, 24, 40, 0.18);
        border-right: 1px solid var(--border);
    }
    .sidebar.open {
        transform: translateX(0);
    }
    .sidebar-close {
        display: flex;
        align-items: center;
        justify-content: center;
    }

    /* 内容区留全宽 */
    .content {
        padding: 18px 14px;
        overflow: visible;
    }

    /* 遮罩显示 */
    .scrim {
        display: block;
    }
}

/* 超小屏微调 */
@media (max-width: 480px) {
    .content {
        padding: 14px 10px;
    }
    .card {
        padding: 16px 14px;
        border-radius: 14px;
    }
    h2 {
        font-size: 20px;
        margin-bottom: 14px;
    }
}

/* ===== 白色侧边栏 ===== */
.sidebar {
    width: 230px;
    padding: 26px 18px;
    display: flex;
    flex-direction: column;
    gap: 26px;
    background: var(--card);
    border-right: 1px solid var(--border);
    position: sticky;
    top: 0;
    height: 100vh;
    overflow-y: auto;
}

.logo {
    margin: 0;
    font-size: 21px;
    font-weight: 800;
    color: var(--text);
}

.sidebar nav {
    display: flex;
    flex-direction: column;
    gap: 8px;
}

.sidebar nav a {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--text-soft);
    text-decoration: none;
    padding: 11px 14px;
    border-radius: 12px;
    transition: all 0.2s;
}

.nav-ico {
    font-size: 16px;
}

.sidebar nav a:hover {
    background: #f3f4f6;
    transform: translateX(3px);
}

.sidebar nav a.router-link-active {
    color: var(--blue);
    background: var(--bg-blue);
    font-weight: 600;
}

.sidebar-bottom {
    margin-top: auto;
    display: flex;
    flex-direction: column;
    gap: 10px;
}

.sidebar-bottom input {
    padding: 10px 12px;
    border-radius: 10px;
    border: 1px solid var(--border);
    background: #fafbfc;
    color: var(--text);
    outline: none;
    transition: border-color 0.2s;
}

.sidebar-bottom input:focus {
    border-color: var(--blue);
}

.sidebar-bottom button {
    padding: 10px;
    border: none;
    border-radius: 10px;
    cursor: pointer;
    font-weight: 600;
    color: #fff;
    background: var(--blue);
    transition: opacity 0.15s;
}

.sidebar-bottom button:hover {
    opacity: 0.9;
}

.logout-btn {
    width: 100%;
    color: var(--text-soft) !important;
    background: #f3f4f6 !important;
    border: 1px solid var(--border) !important;
}

/* ===== 登录页 ===== */
.login-page {
    min-height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 20px;
}

.login-card {
    width: 100%;
    max-width: 400px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 20px;
    padding: 32px 28px;
    box-shadow: 0 8px 30px rgba(16, 24, 40, 0.1);
    display: flex;
    flex-direction: column;
    gap: 14px;
}

.login-card .logo {
    text-align: center;
    font-size: 26px;
}

.login-desc {
    color: var(--text-soft);
    font-size: 13px;
    line-height: 1.6;
    margin: 0;
}

.login-desc code {
    background: var(--bg-violet);
    color: var(--violet);
    padding: 1px 6px;
    border-radius: 6px;
}

.login-card input {
    padding: 12px 14px;
    border-radius: 12px;
    border: 1px solid var(--border);
    background: #fafbfc;
    color: var(--text);
    outline: none;
    font-size: 14px;
}

.login-card input:focus {
    border-color: var(--blue);
    background: #fff;
}

.login-status {
    color: var(--text-soft);
    text-align: center;
    padding: 8px 0;
}

.login-error {
    color: var(--red);
    background: #fdf1f3;
    border-radius: 10px;
    padding: 10px 12px;
    font-size: 13px;
}

.connect-btn {
    width: 100%;
    padding: 12px;
    font-size: 15px;
}

/* ===== 内容区域 ===== */
.content {
    flex: 1;
    padding: 28px 30px;
    overflow: auto;
}

/* ===== 通用白色卡片 ===== */
.card {
    position: relative;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 18px;
    padding: 22px;
    color: var(--text);
    box-shadow: 0 1px 3px rgba(16, 24, 40, 0.06);
    transition: box-shadow 0.2s;
}

.card:hover {
    box-shadow: 0 8px 24px rgba(16, 24, 40, 0.1);
}

h2 {
    color: var(--text);
    margin: 4px 0 20px;
    font-size: 23px;
    font-weight: 800;
}

h3 {
    margin: 0 0 14px;
    font-size: 16px;
    font-weight: 700;
    color: var(--text);
}

.error {
    color: var(--red);
    background: #fdf1f3 !important;
    border-color: #f6c6cd !important;
}

/* 通用按钮 */
.btn {
    border: none;
    border-radius: 12px;
    padding: 10px 18px;
    cursor: pointer;
    font-weight: 600;
    transition: transform 0.15s, box-shadow 0.15s;
}

.btn:hover {
    transform: translateY(-1px);
}

.btn-primary {
    background: var(--bg-blue);
    color: var(--blue);
}

.btn-danger {
    background: #fdebed;
    color: var(--red);
}

.btn-ghost {
    background: #f3f4f6;
    color: var(--text-soft);
    border: 1px solid var(--border);
}

/* ===== 全局限窄屏补丁：覆盖各 view 内通用横排布局 ===== */
@media (max-width: 640px) {
    /* 搜索栏 / 表单行纵向堆叠 */
    .searchbar,
    .fields {
        flex-direction: column !important;
        align-items: stretch !important;
    }
    .searchbar .btn,
    .fields .btn {
        width: 100% !important;
        flex: none !important;
    }

    /* 头部（标题 + 右侧按钮）包行 */
    .head {
        flex-wrap: wrap !important;
        gap: 10px !important;
    }

    /* tabs / 分类 chips 允许横向滚动，避免挤压换行溢出 */
    .tabs,
    .season-bar {
        flex-wrap: wrap !important;
    }

    /* 详情页头部：封面与信息纵向堆叠 */
    .detail .head {
        flex-direction: column !important;
        align-items: flex-start !important;
    }

    /* 分页整宽 */
    .pager {
        flex-wrap: wrap !important;
        justify-content: center !important;
    }

    /* 模态框适配小屏 */
    .modal {
        width: 92vw !important;
        max-height: 86vh !important;
        overflow-y: auto !important;
    }
}
</style>
