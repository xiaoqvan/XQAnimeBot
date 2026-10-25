// 假 qBittorrent WebUI，只实现本插件用到的接口（测试用，不进生产）。
const http = require("http");
const path = require("path");

const VIDEO = process.env.MOCK_QB_VIDEO || "D:/tmp/xq-test/sample.mkv";
const SIZE = Number(process.env.MOCK_QB_SIZE || 1048576);

function json(res, body) {
  const text = JSON.stringify(body);
  res.writeHead(200, { "Content-Type": "application/json" });
  res.end(text);
}

http.createServer((req, res) => {
  const url = new URL(req.url, "http://127.0.0.1");
  if (req.method === "POST" && url.pathname === "/api/v2/auth/login") {
    res.writeHead(200, { "Set-Cookie": "SID=mock; path=/" });
    res.end("Ok.");
    return;
  }
  if (url.pathname === "/api/v2/app/version") { res.end("v5.0.0"); return; }
  if (url.pathname === "/api/v2/app/webapiVersion") { res.end("2.9.3"); return; }
  if (req.method === "POST" && url.pathname === "/api/v2/torrents/add") { res.end("Ok."); return; }
  if (req.method === "POST" && url.pathname === "/api/v2/torrents/delete") { res.end(""); return; }
  if (url.pathname === "/api/v2/torrents/info") {
    json(res, [{
      hash: "0123456789abcdef0123456789abcdef01234567",
      name: "某番 - 04",
      progress: 1.0,
      state: "uploading",
      size: SIZE,
      save_path: path.dirname(VIDEO),
      content_path: VIDEO,
    }]);
    return;
  }
  if (url.pathname === "/api/v2/torrents/files") {
    json(res, [{ index: 0, name: path.basename(VIDEO), size: SIZE, progress: 1.0, priority: 1 }]);
    return;
  }
  if (url.pathname === "/api/v2/torrents/properties") { json(res, { save_path: path.dirname(VIDEO) }); return; }
  if (url.pathname === "/api/v2/transfer/info") { json(res, { dl_info_speed: 0 }); return; }
  res.writeHead(404); res.end("no");
}).listen(Number(process.env.MOCK_QB_PORT || 8080), "127.0.0.1", () => console.log("mock qb on " + (process.env.MOCK_QB_PORT || 8080)));
