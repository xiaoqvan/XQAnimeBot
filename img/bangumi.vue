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
                    <div class="flex flex-row flex-wrap"
                        style="display: flex; flex-wrap: wrap; max-height: 36px; overflow: hidden;">
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
                        style="font-size: 12px; color: #595959; border-left: 3px solid #ff5c9d; padding-left: 6px; margin-bottom: 8px;">bangumi状态看板</span>
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
            style="display: flex; align-items: center; justify-content: flex-end; height: 20px; margin-top: 4px;">
            <span class="m-0 p-0" style="font-size: 9px; color: #bfbfbf;">info By bgm.tv</span>
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
