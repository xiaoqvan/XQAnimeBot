declare module "parse-torrent" {
  // overload: callback style
  function parseTorrent(
    input: any,
    cb: (err: Error | null, parsed: parseTorrent.Instance) => void
  ): void;

  // overload: promise style
  function parseTorrent(input: any): Promise<parseTorrent.Instance>;

  namespace parseTorrent {
    interface Instance {
      infoHash: string;
      name?: string;
      length?: number;
      files?: { path: string; length: number }[];
    }

    function remote(
      torrentId: string,
      opts: { timeout?: number },
      cb: (err: Error | null, parsed: Instance) => void
    ): void;

    function toMagnetURI(parsed: Instance): string;
  }

  export = parseTorrent;
}
