<template>
    <div class="flex flex-col w-full h-full bg-white"
        style="display: flex; flex-direction: column; position: relative; font-family: 'Noto Sans SC', sans-serif; padding: 24px; box-sizing: border-box; justify-content: space-between;">
        <!-- TOP HEADER BAR -->
        <div class="flex flex-row w-full items-center justify-between"
            style="display: flex; height: 60px; border-bottom: 1px solid #f0f0f0; margin-bottom: 12px;">
            <div class="flex flex-col" style="display: flex; flex-direction: column;">
                <div class="flex flex-row items-center" style="display: flex;">
                    <span class="font-bold m-0 p-0" style="font-size: 24px; color: #1f1f1f; line-height: 30px;">{{
                        name_cn || name }}</span>
                    <span class="font-bold m-0 p-0"
                        style="font-size: 10px; color: #ff5c9d; background-color: rgba(255,92,157,0.08); border: 1.5px solid rgba(255,92,157,0.2); border-radius: 4px; padding: 1px 6px; margin-left: 10px;">{{
                            platform }}</span>
                </div>
                <span class="m-0 p-0"
                    style="font-size: 11px; color: #8c8c8c; font-weight: 400; line-height: 14px; margin-top: 1px;">{{
                        name }}</span>
            </div>
            <div class="flex flex-row items-center"
                style="display: flex; border: 1.5px solid #b7eb8f; background-color: #f6ffed; border-radius: 6px; padding: 4px 10px;">
                <span class="font-semibold m-0 p-0" style="font-size: 11px; color: #389e0d;">放送开始：{{ infoboxMap['放送开始']
                    || date }}</span>
            </div>
        </div>

        <!-- MAIN WORKSPACE -->
        <div class="flex flex-row w-full" style="display: flex; height: 420px; justify-content: space-between;">

            <!-- COLUMN 1: Poster + Staff Info -->
            <div class="flex flex-col"
                style="display: flex; flex-direction: column; width: 230px; height: 100%; justify-content: space-between; border-right: 1.5px dashed #f0f0f0; padding-right: 20px; box-sizing: border-box;">
                <div class="flex"
                    style="display: flex; width: 210px; height: 290px; border-radius: 12px; overflow: hidden; background-color: #f5f5f5; border: 1.5px solid #e8e8e8;">
                    <img :src="images.large || images.medium || images.common"
                        style="width: 210px; height: 290px; object-fit: cover;" alt="Poster" />
                </div>

                <!-- Staff metadata -->
                <div class="flex flex-col"
                    style="display: flex; flex-direction: column; background-color: #fafafa; border-radius: 8px; padding: 8px; border: 1px solid #f0f0f0; margin-top: 8px;">
                    <div class="flex flex-row flex-wrap w-full" style="display: flex; flex-wrap: wrap;">
                        <div v-for="item in displayStaff" :key="item.key" class="flex flex-col"
                            style="display: flex; flex-direction: column; width: 50%; margin-bottom: 4px;"
                            :style="(item.align === 'right' ? 'align-items: flex-end;' : '') + '; display: flex; flex-direction: column;'">
                            <span class="m-0 p-0" style="font-size: 9px; color: #bfbfbf;">{{ item.key }}</span>
                            <span class="font-medium m-0 p-0"
                                style="font-size: 10px; color: #434343; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; max-width: 100px;">{{
                                    item.value || '-' }}</span>
                        </div>
                    </div>
                </div>
            </div>

            <!-- COLUMN 2: Episode Grid + Schedule + Tags -->
            <div class="flex flex-col"
                style="display: flex; flex-direction: column; width: 450px; height: 100%; justify-content: flex-start; border-right: 1.5px dashed #f0f0f0; padding-right: 20px; box-sizing: border-box; margin-left: 20px;">
                <!-- Episode Grid Container -->
                <div class="flex flex-col" style="display: flex; flex-direction: column; margin-bottom: 6px;">
                    <div class="flex flex-row items-center justify-between" style="display: flex; margin-bottom: 6px;">
                        <div class="flex flex-row items-center" style="display: flex;">
                            <span class="font-bold m-0 p-0"
                                style="font-size: 13px; color: #595959; border-left: 3px solid #ff5c9d; padding-left: 6px;">章节列表</span>
                            <!-- Pagination Tabs -->
                            <div class="flex flex-row" style="display: flex; margin-left: 8px;">
                                <div v-for="p in Math.ceil(episodes.length / episodesPerPage)" :key="p"
                                    class="flex flex-row items-center justify-center"
                                    style="display: flex; height: 18px; padding: 0 6px; border-radius: 4px; margin-right: 4px; box-sizing: border-box;"
                                    :style="{
                                        backgroundColor: (p - 1) === activePageIdx ? '#ff5c9d' : '#f0f0f0',
                                        border: (p - 1) === activePageIdx ? '1px solid #ff5c9d' : '1px solid #d9d9d9'
                                    }">
                                    <span class="m-0 p-0" style="font-size: 9px; font-weight: 700;" :style="{
                                        color: (p - 1) === activePageIdx ? '#ffffff' : '#595959',
                                        fontWeight: (p - 1) === activePageIdx ? '700' : '400'
                                    }">{{ String((p - 1) * episodesPerPage + 1).padStart(2, '0') }}-{{
                                        String(Math.min(p * episodesPerPage, episodes.length)).padStart(2, '0')
                                    }}</span>
                                </div>
                            </div>
                        </div>
                        <span class="font-medium m-0 p-0" style="font-size: 10px; color: #8c8c8c;">进度: {{
                            airedCount }}/{{ episodes.length }}</span>
                    </div>
                    <!-- Fixed Height Grid (3 Rows, 10 per row) -->
                    <div class="flex flex-row flex-wrap"
                        style="display: flex; flex-wrap: wrap; width: 400px; height: 120px;">
                        <div v-for="ep in episodes.slice(activePageIdx * episodesPerPage, activePageIdx * episodesPerPage + episodesPerPage)"
                            :key="ep.ep" class="flex flex-row items-center justify-center"
                            style="display: flex; width: 34px; height: 34px; border-radius: 6px; margin-right: 6px; margin-bottom: 6px; box-sizing: border-box;"
                            :style="{
                                backgroundColor: ep.sort <= currentEpNum ? 'rgba(255,92,157,0.06)' : '#f5f5f5',
                                border: ep.sort <= currentEpNum ? '1.5px solid #ff5c9d' : '1.5px solid #e8e8e8'
                            }">
                            <span class="m-0 p-0" style="font-size: 13px; font-weight: 600;" :style="{
                                color: ep.sort <= currentEpNum ? '#ff5c9d' : '#8c8c8c',
                                fontWeight: ep.sort <= currentEpNum ? '600' : '400'
                            }">{{ ep.ep < 10 ? '0' + ep.ep : ep.ep }}</span>
                        </div>
                        <!-- Fill empty slots -->
                        <div v-for="n in (episodesPerPage - Math.min(Math.max(0, episodes.length - activePageIdx * episodesPerPage), episodesPerPage))"
                            :key="'empty-' + n" class="flex"
                            style="display: flex; width: 34px; height: 34px; margin-right: 6px; margin-bottom: 6px; box-sizing: border-box; opacity: 0;">
                        </div>
                    </div>
                </div>

                <!-- AIR DATE PREDICTOR -->
                <div class="flex flex-row items-center"
                    style="display: flex; background-color: rgba(255,92,157,0.03); border: 1px solid rgba(255,92,157,0.12); border-radius: 8px; padding: 8px 14px; margin-bottom: 10px; box-sizing: border-box; width: 430px; justify-content: space-between;">
                    <div class="flex flex-col"
                        style="display: flex; flex-direction: column; width: 195px; border-right: 1px dashed rgba(255,92,157,0.12); padding-right: 10px; box-sizing: border-box;">
                        <span class="m-0 p-0"
                            style="font-size: 10px; color: #8c8c8c; margin-bottom: 2px; font-weight: 500;">当前进度 第 <span
                                style="color: #ff5c9d; font-weight: bold;">{{ currentEpNum || '-' }}</span> 话</span>
                        <span class="m-0 p-0"
                            style="font-size: 11px; color: #262626; font-weight: 600; font-family: monospace;">{{
                                currentEpAirDate || '' }} <span v-if="currentEpName"
                                style="font-size: 9px; color: #8c8c8c; font-family: 'Noto Sans SC', sans-serif;">{{
                                    currentEpName }}</span></span>
                    </div>
                    <div class="flex flex-col"
                        style="display: flex; flex-direction: column; width: 195px; padding-left: 10px; box-sizing: border-box;">
                        <template v-if="airedCount >= episodes.length">
                            <span class="m-0 p-0"
                                style="font-size: 12px; color: #ff5c9d; font-weight: 700; letter-spacing: 1px;">
                                已完结</span>
                            <span class="m-0 p-0" style="font-size: 10px; color: #8c8c8c; margin-top: 2px;">共 {{
                                episodes.length }} 话</span>
                        </template>
                        <template v-else>
                            <span class="m-0 p-0"
                                style="font-size: 10px; color: #8c8c8c; margin-bottom: 2px; font-weight: 500;">下一集预定 第
                                <span style="color: #ff5c9d; font-weight: bold;">{{ nextEpNum || '-' }}</span> 话</span>
                            <span class="m-0 p-0"
                                style="font-size: 11px; color: #ff5c9d; font-weight: 600; font-family: monospace;">{{
                                    nextEpAirDate || '' }} <span v-if="nextEpName"
                                    style="font-size: 9px; color: #8c8c8c; font-family: 'Noto Sans SC', sans-serif;">{{
                                        nextEpName }}</span></span>
                        </template>
                    </div>
                </div>

                <!-- Other Episodes (非本篇) -->
                <div v-if="otherEpisodes.length > 0" class="flex flex-col"
                    style="display: flex; flex-direction: column; margin-bottom: 6px;">
                    <span class="font-bold m-0 p-0"
                        style="font-size: 13px; color: #595959; border-left: 3px solid #ff5c9d; padding-left: 6px; margin-bottom: 6px;">其他集数</span>
                    <div class="flex flex-row flex-wrap" style="display: flex; flex-wrap: wrap;">
                        <template v-for="(group, gi) in otherEpisodes" :key="'g-' + group.type">
                            <!-- Green vertical divider before each group -->
                            <div
                                style="display: flex; width: 1.5px; height: 24px; margin-right: 6px; margin-bottom: 6px; border-radius: 1px; background-color: #b7eb8f; flex-shrink: 0;">
                            </div>
                            <!-- Type label (text only) -->
                            <span class="font-bold m-0 p-0"
                                style="font-size: 11px; color: #52c41a; line-height: 24px; height: 24px; margin-right: 6px; margin-bottom: 6px;">{{
                                    group.label }}</span>
                            <!-- Episode number blocks -->
                            <div v-for="ep in group.episodes" :key="ep.id"
                                class="flex flex-row items-center justify-center"
                                style="display: flex; width: 26px; height: 26px; border-radius: 5px; margin-right: 6px; margin-bottom: 6px; box-sizing: border-box; background-color: #f5f5f5; border: 1.5px solid #e8e8e8;">
                                <span class="m-0 p-0" style="font-size: 11px; font-weight: 500; color: #8c8c8c;">{{
                                    ep.ep < 10 ? '0' + ep.ep : ep.ep }}</span>
                            </div>
                        </template>
                    </div>
                </div>

                <!-- Tags -->
                <div class="flex flex-col" style="display: flex; flex-direction: column;">
                    <span class="font-bold m-0 p-0"
                        style="font-size: 13px; color: #595959; border-left: 3px solid #ff5c9d; padding-left: 6px; margin-bottom: 6px;">热门标签</span>
                    <div class="flex flex-row flex-wrap"
                        style="display: flex; flex-wrap: wrap; max-height: 80px; overflow: hidden;">
                        <div v-for="(tag, idx) in tags" :key="idx" class="flex flex-row items-center"
                            style="display: flex; border-radius: 9999px; padding: 2px 10px; margin-right: 6px; margin-bottom: 5px; height: 22px; box-sizing: border-box;"
                            :style="{
                                backgroundColor: idx === 0 ? 'rgba(255,92,157,0.06)' : '#fbfbfb',
                                border: idx === 0 ? '1.5px solid rgba(255,92,157,0.2)' : '1.5px solid #d9d9d9'
                            }">
                            <span class="font-medium m-0 p-0" style="font-size: 11px; line-height: 12px;"
                                :style="{ color: idx === 0 ? '#ff5c9d' : '#595959' }">{{ tag.name }}</span>
                        </div>
                    </div>
                </div>

                <!-- Fansubs（活跃字幕组） -->
                <div v-if="fansubs && fansubs.length > 0" class="flex flex-col"
                    style="display: flex; flex-direction: column; margin-top: 6px;">
                    <span class="font-bold m-0 p-0"
                        style="font-size: 13px; color: #595959; border-left: 3px solid #52c41a; padding-left: 6px; margin-bottom: 6px;">字幕组</span>
                    <div class="flex flex-row flex-wrap" style="display: flex; flex-wrap: wrap;">
                        <div v-for="(fansub, idx) in fansubs" :key="idx" class="flex flex-row items-center"
                            style="display: flex; border-radius: 9999px; padding: 2px 10px; margin-right: 6px; margin-bottom: 5px; height: 22px; box-sizing: border-box; background-color: rgba(82,196,26,0.06); border: 1.5px solid rgba(82,196,26,0.2);">
                            <span class="font-medium m-0 p-0"
                                style="font-size: 11px; line-height: 12px; color: #52c41a;">{{ fansub
                                }}</span>
                        </div>
                    </div>
                </div>
            </div>

            <!-- COLUMN 3: Rating + Collection -->
            <div class="flex flex-col"
                style="display: flex; flex-direction: column; width: 232px; height: 100%; box-sizing: border-box; margin-left: 20px;">

                <!-- Rating Panel -->
                <div class="flex flex-col w-full"
                    style="display: flex; flex-direction: column; background-color: #ffffff; border: 1.5px solid #f0f0f0; border-radius: 16px; padding: 12px 12px 10px; box-sizing: border-box; box-shadow: 0 4px 14px rgba(0,0,0,0.03);">
                    <!-- Score Header -->
                    <div class="flex flex-row items-center"
                        style="display: flex; margin-bottom: 4px; position: relative;">
                        <span class="m-0 p-0"
                            style="position: absolute; top: -6px; right: 0; font-size: 9px; color: #bfbfbf; line-height: 9px;">{{
                                generatedAt }}</span>
                        <!-- Rating Emoji (sprite: 5 icons, left=worst, right=best) -->
                        <div
                            :style="'display: flex; width: ' + (rateEmoW / 5) + 'px; height: ' + rateEmoH + 'px; margin-right: 8px; flex-shrink: 0; overflow: hidden;'">
                            <img :src="rateEmoDataUri"
                                :style="'display: flex; width: ' + rateEmoW + 'px; height: ' + rateEmoH + 'px; max-width: none; flex-shrink: 0; margin-left: -' + (ratingEmojiIdx * rateEmoW / 5) + 'px;'" />
                        </div>
                        <div class="flex flex-col justify-center"
                            style="display: flex; flex-direction: column; justify-content: center;">
                            <div class="flex flex-row items-baseline" style="display: flex;">
                                <span class="font-bold m-0 p-0"
                                    style="font-size: 20px; color: #ff5c9d; margin-right: 4px; line-height: 20px;">{{
                                        rating.score.toFixed(1) }}</span>
                                <span class="font-bold m-0 p-0"
                                    style="font-size: 11px; color: #ff5c9d; line-height: 11px;">{{
                                        ratingLabel }}</span>
                            </div>
                        </div>
                    </div>

                    <!-- Rating Histogram -->
                    <div class="flex flex-col"
                        style="display: flex; flex-direction: column; background-color: #f5f5f5; border-radius: 10px; padding: 6px 8px; box-sizing: border-box;">
                        <div class="flex flex-row justify-end w-full" style="display: flex; margin-bottom: 4px;">
                            <span class="m-0 p-0" style="font-size: 9px; color: #a0a0a0; font-weight: 400;">{{
                                rating.total }} votes</span>
                        </div>
                        <div class="flex flex-row w-full items-end justify-between"
                            style="display: flex; height: 70px;">
                            <div v-for="(pct, idx) in barPcts" :key="idx" class="flex flex-col items-center"
                                style="display: flex; flex-direction: column; align-items: center; width: 14px; height: 100%;">
                                <div
                                    style="flex: 1; min-height: 0; width: 100%; display: flex; flex-direction: column; justify-content: flex-end; align-items: center;">
                                    <div style="display: flex; width: 12px; border-radius: 2px 2px 0 0;"
                                        :style="'height: ' + pct + '%; ' + (idx === 10 - Math.round(rating.score) ? 'background-color: #ff5c9d;' : 'background-color: #a6a6a6;')">
                                    </div>
                                </div>
                                <span class="m-0 p-0"
                                    style="font-size: 8px; color: #8c8c8c; font-weight: 400; line-height: 8px; margin-top: 3px;">{{
                                        10 - idx }}</span>
                            </div>
                        </div>
                    </div>
                </div>

                <!-- Collection Stats -->
                <div class="flex flex-col w-full"
                    style="display: flex; flex-direction: column; height: 145px; margin-top: 10px; background-color: #ffffff; border: 1.5px solid #f0f0f0; border-radius: 16px; padding: 12px; box-sizing: border-box; box-shadow: 0 4px 14px rgba(0,0,0,0.03); justify-content: flex-start;">
                    <span class="font-bold m-0 p-0"
                        style="font-size: 12px; color: #595959; border-left: 3px solid #ff5c9d; padding-left: 6px; margin-bottom: 8px;">bangumi用户收藏状态</span>
                    <div class="flex flex-col" style="display: flex; flex-direction: column; gap: 4px;">
                        <div class="flex flex-row items-center justify-between"
                            style="display: flex; height: 24px; padding: 0 4px;">
                            <div class="flex flex-row items-center" style="display: flex;">
                                <div class="flex items-center justify-center rounded"
                                    style="display: flex; width: 8px; height: 8px; background-color: #faad14; margin-right: 6px;">
                                </div>
                                <span class="m-0 p-0" style="font-size: 11px; color: #595959;">想看</span>
                            </div>
                            <span class="font-bold m-0 p-0" style="font-size: 12px; color: #faad14;">{{
                                collection.wish_fmt }}</span>
                        </div>
                        <div class="flex flex-row items-center justify-between"
                            style="display: flex; height: 24px; padding: 0 4px;">
                            <div class="flex flex-row items-center" style="display: flex;">
                                <div class="flex items-center justify-center rounded"
                                    style="display: flex; width: 8px; height: 8px; background-color: #52c41a; margin-right: 6px;">
                                </div>
                                <span class="m-0 p-0" style="font-size: 11px; color: #595959;">看过</span>
                            </div>
                            <span class="font-bold m-0 p-0" style="font-size: 12px; color: #52c41a;">{{
                                collection.collect_fmt }}</span>
                        </div>
                        <div class="flex flex-row items-center justify-between"
                            style="display: flex; height: 24px; padding: 0 4px;">
                            <div class="flex flex-row items-center" style="display: flex;">
                                <div class="flex items-center justify-center rounded"
                                    style="display: flex; width: 8px; height: 8px; background-color: #1677ff; margin-right: 6px;">
                                </div>
                                <span class="m-0 p-0" style="font-size: 11px; color: #595959;">在看</span>
                            </div>
                            <span class="font-bold m-0 p-0" style="font-size: 12px; color: #1677ff;">{{
                                collection.doing_fmt }}</span>
                        </div>
                        <div class="flex flex-row items-center justify-between"
                            style="display: flex; height: 24px; padding: 0 4px;">
                            <div class="flex flex-row items-center" style="display: flex;">
                                <div class="flex items-center justify-center rounded"
                                    style="display: flex; width: 8px; height: 8px; background-color: #fa8c16; margin-right: 6px;">
                                </div>
                                <span class="m-0 p-0" style="font-size: 11px; color: #595959;">搁置</span>
                            </div>
                            <span class="font-bold m-0 p-0" style="font-size: 12px; color: #fa8c16;">{{
                                collection.on_hold_fmt }}</span>
                        </div>
                    </div>
                </div>

            </div>
        </div>

        <!-- Bottom Bar -->
        <div class="flex flex-row"
            style="display: flex; align-items: center; justify-content: space-between; height: 20px; margin-top: 4px;">
            <span style="display: flex; align-items: center; gap: 4px;">
                <img src="data:image/jpeg;base64,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"
                    style="height: 16px; width: 16px; border-radius: 50%; object-fit: cover; flex-shrink: 0;" />
                <span class="m-0 p-0" style="font-size: 9px; color: #bfbfbf;">@XQ_Anime / XiaoQvan的动画仓库</span>
            </span>
            <span style="display: flex; align-items: center; gap: 3px;">
                <span class="m-0 p-0" style="font-size: 9px; color: #bfbfbf;">数据来自</span>
                <img src="data:image/png;base64,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"
                    style="height: 14px; vertical-align: middle;" /></span>
        </div>
        <!-- Bottom Border -->
        <div class="flex absolute"
            style="display: flex; bottom: 0; left: 0; right: 0; height: 4px; background-color: #ff5c9d;">
        </div>
    </div>
</template>

<script setup lang="ts">
defineProps<{
    id: number
    name: string
    name_cn: string
    images: { small?: string; grid?: string; large?: string; medium?: string; common?: string }
    date: string
    platform: string
    total_episodes: number
    rating: {
        rank: number
        total: number
        count: Record<number, number>
        score: number
    }
    tags: { name: string; count: number }[]
    infoboxMap: Record<string, string>
    collection: { on_hold: number; dropped: number; wish: number; collect: number; doing: number; on_hold_fmt: string; wish_fmt: string; collect_fmt: string; doing_fmt: string }
    displayStaff: { key: string; value: string; align?: string }[]
    ratingLabel: string
    ratingEmojiIdx: number
    generatedAt: string
    rateEmoDataUri: string
    rateEmoW: number
    rateEmoH: number
    barPcts: number[]
    episodes: { ep: number; name: string; name_cn: string; airdate: string; sort: number; type: number }[]
    otherEpisodes: { type: number; label: string; episodes: { id: number; ep: number; name: string; name_cn: string; airdate: string; sort: number; type: number }[] }[]
    airedCount: number
    episodesPerPage: number
    activePageIdx: number
    currentEpNum: number
    currentEpName: string
    currentEpAirDate: string
    nextEpNum: number
    nextEpName: string
    nextEpAirDate: string
    fansubs: string[]
}>()
</script>
