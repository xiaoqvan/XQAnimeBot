import { parseInfo } from "../utils/animeParser.ts";

async function subtest(title: string, teamName: string) {
  const result = parseInfo(title, teamName);
  console.log(result);
}

subtest(
  "[樱桃花字幕组]转生为第七王子，随心所欲的魔法学习之路 第二季 Dainanaoji S2 - 13 [1080p][简日内嵌]",
  "樱桃花字幕组"
);
