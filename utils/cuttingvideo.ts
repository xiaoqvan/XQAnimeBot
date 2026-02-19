import logger from "@log/index.ts"
import { spawn } from "node:child_process"
import { stat, rm, mkdir, readdir } from "node:fs/promises"
import { resolve, join } from "node:path"

/**
 * 视频分割选项配置
 */
interface SplitOptions {
    /** 输入视频文件路径 */
    input: string
    /** 输出目录路径，默认为当前目录 */
    outDir?: string
    /** 每个分段的最大大小(GiB)，默认为1.95 */
    maxGiB?: number
    /** 每个分段的最小时长(秒)，默认为10 */
    minSegmentSec?: number
    /** 最大重试次数，默认为6 */
    maxRetries?: number
}

/**
 * 运行命令行程序并返回输出
 * @param cmd - 要执行的命令
 * @param args - 命令参数数组
 * @returns Promise，包含stdout和stderr的对象
 */
function run(cmd: string, args: string[]): Promise<{ stdout: string; stderr: string }> {
    return new Promise((resolvePromise, reject) => {
        const child = spawn(cmd, args)

        let stdout = ""
        let stderr = ""

        child.stdout.on("data", (d) => (stdout += d.toString()))
        child.stderr.on("data", (d) => (stderr += d.toString()))

        child.on("error", reject)

        child.on("close", (code) => {
            if (code === 0) {
                resolvePromise({ stdout, stderr })
            } else {
                reject(new Error(stderr || `Process exited with code ${code}`))
            }
        })
    })
}

/**
 * 获取视频时长(秒)
 * @param input - 输入视频文件路径
 * @returns Promise，返回视频时长(秒)
 */
async function getDurationSeconds(input: string): Promise<number> {
    const { stdout } = await run("ffprobe", [
        "-v",
        "error",
        "-show_entries",
        "format=duration",
        "-of",
        "default=noprint_wrappers=1:nokey=1",
        input,
    ])

    return parseFloat(stdout.trim())
}

/**
 * 列出指定目录中以特定前缀开头的所有文件
 * @param outDir - 输出目录路径
 * @param prefix - 文件名前缀
 * @returns Promise，返回文件路径数组
 */
async function listParts(outDir: string, prefix: string) {
    const files = await readdir(outDir)
    return files
        .filter((f) => f.startsWith(prefix))
        .map((f) => join(outDir, f))
}

/**
 * 将视频按最大文件大小分割成多个片段
 * 自动计算合适的分段时长，确保每个分段不超过指定大小
 * 如果有分段超过大小限制，会自动缩短分段时间并重试
 * @param options - 分割选项配置
 * @returns Promise，返回生成的所有分段文件路径数组
 * @throws 如果视频时长无效、未生成分段文件或多次重试后仍有分段超过上限
 */
export async function splitVideoByMaxSize(options: SplitOptions) {
    const {
        input,
        outDir = ".",
        maxGiB = 1.95,
        minSegmentSec = 10,
        maxRetries = 6,
    } = options

    const inputPath = resolve(input)
    const outputDir = resolve(outDir)

    await mkdir(outputDir, { recursive: true })

    const fileStat = await stat(inputPath)
    const duration = await getDurationSeconds(inputPath)

    if (!duration || duration <= 0) {
        throw new Error("视频时长无效")
    }

    const avgBytesPerSec = fileStat.size / duration
    const maxBytes = maxGiB * 1024 ** 3

    let segmentSec = Math.max(
        minSegmentSec,
        Math.floor((maxBytes * 0.98) / avgBytesPerSec),
    )

    const baseName = inputPath.split(/[/\\]/).pop()!.replace(/\.[^/.]+$/, "")
    const prefix = `${baseName}_part_`
    const outputPattern = join(outputDir, `${prefix}%03d.mp4`)

    for (let attempt = 1; attempt <= maxRetries; attempt++) {
        logger.info(`[TRY ${attempt}] 分段时长 ${segmentSec}s`)

        // 删除旧分段
        const oldParts = await listParts(outputDir, prefix)
        await Promise.all(oldParts.map((f) => rm(f, { force: true })))

        await run("ffmpeg", [
            "-hide_banner",
            "-y",
            "-i",
            inputPath,
            "-c",
            "copy",
            "-map",
            "0",
            "-f",
            "segment",
            "-segment_time",
            String(segmentSec),
            "-reset_timestamps",
            "1",
            outputPattern,
        ])

        const parts = await listParts(outputDir, prefix)
        if (!parts.length) {
            throw new Error("未生成任何分段文件")
        }

        const overs = []
        for (const p of parts) {
            const s = await stat(p)
            if (s.size > maxBytes) overs.push(p)
        }

        if (!overs.length) {
            logger.info(`成功，共 ${parts.length} 段`)
            return parts
        }

        logger.info(`有 ${overs.length} 段超出大小，缩短分段时间重试`)
        segmentSec = Math.max(minSegmentSec, Math.floor(segmentSec * 0.85))
    }

    throw new Error("多次尝试仍有分段超过上限")
}
