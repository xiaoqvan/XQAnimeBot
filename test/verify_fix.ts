
import { parseInfo } from "../utils/animeParser.ts";

const rawTitle = "[晚街与灯][命运-奇异赝品_Fate strange Fake][03 没有英灵的战斗][WebRip][1080P_AVC_AAC][简日双语内嵌]";
const teamName = "晚街与灯";

const result = parseInfo(rawTitle, teamName) || {};

console.log("Raw Title:", rawTitle);
console.log("Parsed Name:", result.name);
console.log("Parsed Episode:", result.episode);

if (result.episode && result.episode[0] === "03") {
    console.log("SUCCESS: Episode extracted correctly.");
} else {
    console.log("FAILURE: Episode extraction failed. Got:", result.episode);
}
