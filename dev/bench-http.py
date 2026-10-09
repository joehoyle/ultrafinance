#!/usr/bin/env python3
"""Closed-loop local HTTP enrichment benchmark; uses only Python's standard library."""
import argparse
import asyncio
import collections
import datetime
import json
import math
import time
import urllib.parse


async def benchmark(args):
    url = urllib.parse.urlsplit(args.url)
    if url.scheme != "http" or url.hostname not in ("localhost", "127.0.0.1", "::1"):
        raise ValueError("benchmark target must be a local HTTP server")
    payload = args.body.encode()
    path = (url.path or "/") + (f"?{url.query}" if url.query else "")
    wire = (f"POST {path} HTTP/1.1\r\nHost: {url.netloc}\r\n"
            f"Content-Type: application/json\r\nContent-Length: {len(payload)}\r\n"
            "Connection: keep-alive\r\n\r\n").encode() + payload
    results = []
    next_request = 0

    async def worker():
        nonlocal next_request
        reader = writer = None
        try:
            while next_request < args.requests:
                next_request += 1
                started = time.perf_counter()
                try:
                    async with asyncio.timeout(60):
                        if writer is None:
                            reader, writer = await asyncio.open_connection(url.hostname, url.port or 80)
                        writer.write(wire)
                        await writer.drain()
                        headers = (await reader.readuntil(b"\r\n\r\n")).decode().split("\r\n")
                        status = int(headers[0].split()[1])
                        fields = dict(line.lower().split(":", 1) for line in headers[1:] if ":" in line)
                        if "content-length" in fields:
                            body = await reader.readexactly(int(fields["content-length"]))
                        elif fields.get("transfer-encoding", "").strip() == "chunked":
                            chunks = []
                            while True:
                                size = int((await reader.readline()).split(b";")[0], 16)
                                if not size:
                                    while await reader.readline() != b"\r\n":
                                        pass
                                    break
                                chunks.append(await reader.readexactly(size))
                                await reader.readexactly(2)
                            body = b"".join(chunks)
                        else:
                            raise ValueError("response has no supported body framing")
                        data = json.loads(body)
                        merchant = data.get("merchant", {}).get("status")
                        valid = status == 200 and merchant in ("matched", "unresolved") and isinstance(data.get("location"), dict)
                        results.append((time.perf_counter() - started, status, merchant, valid))
                        if fields.get("connection", "").strip() == "close":
                            writer.close()
                            await writer.wait_closed()
                            reader = writer = None
                except Exception as error:
                    results.append((time.perf_counter() - started, type(error).__name__, None, False))
                    if writer:
                        writer.close()
                    reader = writer = None
        finally:
            if writer:
                writer.close()
                await writer.wait_closed()

    started_at = datetime.datetime.now(datetime.timezone.utc).isoformat()
    started = time.perf_counter()
    await asyncio.gather(*(worker() for _ in range(args.concurrency)))
    elapsed = time.perf_counter() - started
    latencies = sorted(r[0] * 1000 for r in results)
    successful = sum(r[3] for r in results)
    output = {
        "started_at": started_at,
        "finished_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "url": args.url, "request": json.loads(args.body),
        "concurrency": args.concurrency, "requests": len(results),
        "successful": successful, "errors": len(results) - successful,
        "elapsed_s": round(elapsed, 3), "successful_req_s": round(successful / elapsed, 3),
        "latency_ms": {"mean": round(sum(latencies) / len(latencies), 2),
                       **{f"p{p}": round(latencies[math.ceil(len(latencies) * p / 100) - 1], 2)
                          for p in (50, 95, 99)}},
        "http_statuses": dict(collections.Counter(str(r[1]) for r in results)),
        "merchant_statuses": dict(collections.Counter(r[2] for r in results if r[2])),
    }
    print(json.dumps(output, indent=2), flush=True)
    if args.output:
        with open(args.output, "w") as file:
            json.dump(output, file, indent=2)
            file.write("\n")
    return bool(output["errors"])


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="http://127.0.0.1:3001/v1/enrich")
    parser.add_argument("--body", default='{"description":"SQ* JULIUS BROMONT"}')
    parser.add_argument("--concurrency", type=int, default=1)
    parser.add_argument("--requests", type=int, default=16)
    parser.add_argument("--output")
    args = parser.parse_args()
    if args.concurrency < 1 or args.requests < 1:
        parser.error("concurrency and requests must be positive")
    json.loads(args.body)
    raise SystemExit(asyncio.run(benchmark(args)))
