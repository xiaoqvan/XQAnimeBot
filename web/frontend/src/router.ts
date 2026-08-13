import type { RouteRecordRaw } from "vue-router";

export const routes: RouteRecordRaw[] = [
    {
        path: "/",
        name: "dashboard",
        component: () => import("./views/Dashboard.vue"),
        meta: { title: "概览" },
    },
    {
        path: "/anime",
        name: "anime-list",
        component: () => import("./views/AnimeList.vue"),
        meta: { title: "动漫库" },
    },
    {
        path: "/anime/:id",
        name: "anime-detail",
        component: () => import("./views/AnimeDetail.vue"),
        meta: { title: "番剧详情" },
    },
    {
        path: "/progress",
        name: "progress",
        component: () => import("./views/Progress.vue"),
        meta: { title: "处理进度" },
    },
    {
        path: "/torrents",
        name: "torrents",
        component: () => import("./views/Torrents.vue"),
        meta: { title: "BT 下载" },
    },
    {
        path: "/tasks",
        name: "tasks",
        component: () => import("./views/Tasks.vue"),
        meta: { title: "BT 任务" },
    },
    {
        path: "/reviews",
        name: "reviews",
        component: () => import("./views/Reviews.vue"),
        meta: { title: "待确认番剧" },
    },
    {
        path: "/ai",
        name: "ai",
        component: () => import("./views/AiLog.vue"),
        meta: { title: "AI 调用记录" },
    },
];
