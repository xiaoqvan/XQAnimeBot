import { createApp } from "vue";
import { createRouter, createWebHashHistory } from "vue-router";
import App from "./App.vue";
import { routes } from "./router.ts";

// 使用 hash 模式：兼容后端静态托管（无需服务器端路由回退配置）
const router = createRouter({
    history: createWebHashHistory(),
    routes,
});

router.afterEach((to) => {
    document.title = to.meta.title ? `${String(to.meta.title)} · XQ 动漫` : "XQ 动漫";
});

createApp(App).use(router).mount("#app");
