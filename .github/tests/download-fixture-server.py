"""在宿主回环地址分配空闲端口，仅提供本轮下载夹具。"""

from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer


if __name__ == "__main__":
    with ThreadingHTTPServer(("127.0.0.1", 0), SimpleHTTPRequestHandler) as server:
        print(server.server_port, flush=True)
        server.serve_forever()
