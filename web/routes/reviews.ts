import type { FastifyInstance } from "fastify";
import {
  listPendingReviews,
  getPendingReviewById,
  listCacheAnimes,
} from "../../database/query.ts";
import { updatePendingReviewStatus } from "../../database/update.ts";
import { deletePendingReview } from "../../database/delete.ts";

/** 流程类型：先发后审 / 先审核后发 */
type FlowType = "pre_post" | "pre_review";

function mapAnimeForReview(a: Record<string, unknown>) {
  return {
    id: a.id,
    name: a.name_cn || a.name,
    animeId: a.id,
    title: typeof a.title === "string" ? a.title : `${a.name_cn || a.name}（缓存）`,
    image: a.image,
    episode: a.episode,
    createdAt: a.updatedAt instanceof Date ? a.updatedAt.toISOString() : undefined,
    status: "pending",
    flow: "pre_review" as FlowType,
    flowLabel: "先审核后发",
  };
}

/**
 * 待确认番剧路由
 * - flow=pre_post   （先发后审）：数据来自 pendingReviews 集合
 * - flow=pre_review （先审核后发）：数据来自 cacheAnime（缓存番剧，待转正）
 */
export async function reviewsRoutes(app: FastifyInstance): Promise<void> {
  /** 待确认番剧列表 */
  app.get<{
    Querystring: { status?: string; flow?: string; page?: string; pageSize?: string };
  }>("/api/reviews", async (request, reply) => {
    const flow = (request.query.flow as FlowType | undefined) ?? "pre_post";
    const page = Number(request.query.page) || 1;
    const pageSize = Number(request.query.pageSize) || 30;
    const status = request.query.status ?? "pending";

    if (flow !== "pre_post" && flow !== "pre_review") {
      return reply.code(400).send({ error: "无效的 flow 参数" });
    }
    if (status && !["pending", "approved", "rejected"].includes(status)) {
      return reply.code(400).send({ error: "无效的 status 参数" });
    }

    try {
      // 先审核后发：来自缓存番剧 cacheAnime（待确认/待转正）
      if (flow === "pre_review") {
        const res = await listCacheAnimes(page, pageSize);
        return {
          flow,
          items: res.items.map(mapAnimeForReview),
          total: res.total,
          page: res.page,
          pageSize: res.pageSize,
        };
      }

      // 先发后审：来自 pendingReviews
      const items = await listPendingReviews(
        (status as "pending" | "approved" | "rejected") || "pending"
      );
      return {
        flow,
        items: items.map((r) => ({
          id: r.id,
          createdAt: r.createdAt.toISOString(),
          status: r.status,
          animeName: r.anime.name_cn || r.anime.name,
          animeId: r.anime.id,
          title: r.item?.title ?? "",
          episodeSort: r.episodeSort,
          episodeId: r.episodeId,
          matchConfidence: r.matchDetail?.confidence,
          matchReason: r.matchDetail?.reason,
          flow: "pre_post" as FlowType,
          flowLabel: "先发后审",
        })),
      };
    } catch (err) {
      return reply.code(500).send({ error: (err as Error).message });
    }
  });

  /** 确认待审核番剧：标记为 approved */
  app.post<{ Params: { id: string } }>(
    "/api/reviews/:id/approve",
    async (request, reply) => {
      const id = Number(request.params.id);
      const review = await getPendingReviewById(id);
      if (!review) {
        return reply.code(404).send({ error: "待审核记录不存在" });
      }
      try {
        await updatePendingReviewStatus(id, "approved");
        return { ok: true, id };
      } catch (err) {
        return reply.code(500).send({ error: (err as Error).message });
      }
    }
  );

  /** 拒绝/纠正待审核番剧：标记为 rejected（或删除记录） */
  app.post<{ Params: { id: string }; Body?: { remove?: boolean } }>(
    "/api/reviews/:id/reject",
    async (request, reply) => {
      const id = Number(request.params.id);
      const review = await getPendingReviewById(id);
      if (!review) {
        return reply.code(404).send({ error: "待审核记录不存在" });
      }
      try {
        // 若要求彻底移除（纠正流程），直接删除记录
        if (request.body?.remove) {
          await deletePendingReview(id);
        } else {
          await updatePendingReviewStatus(id, "rejected");
        }
        return { ok: true, id };
      } catch (err) {
        return reply.code(500).send({ error: (err as Error).message });
      }
    }
  );
}
