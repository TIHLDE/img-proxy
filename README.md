# img-proxy

A small Rust proxy in front of [imagor](https://github.com/cshum/imagor). It checks image requests before passing them on: public images are served as they are, and private images need a signed URL that expires.

## Clone and build

```sh
git clone <repo-url> img-proxy
cd img-proxy
docker build -t img-proxy .
```

To build without Docker, run `cargo build --release`. The binary ends up at `target/release/img-proxy`.

## Run

```sh
docker run -d --name img-proxy -p 8080:8080 \
  -e SIGNING_SECRET="$(openssl rand -hex 32)" \
  -e IMAGOR_URL=http://imagor:8000 \
  img-proxy
```

The proxy has to be able to reach imagor. If imagor runs in another container, put both containers on the same Docker network. The default `IMAGOR_URL` points at `127.0.0.1`, which inside the container means the container itself.

Health check: `GET /healthz` returns `ok`.

## Configuration

All settings are environment variables.

| Variable          | Default                  | Description                                                                                 |
| ----------------- | ------------------------ | ------------------------------------------------------------------------------------------- |
| `SIGNING_SECRET`  | **required**             | HMAC secret for private URLs. Must be at least 32 bytes. Your backend needs the same value. |
| `IMAGOR_URL`      | `http://127.0.0.1:8000`  | Base URL of the internal imagor instance. Only plain HTTP is supported.                     |
| `LISTEN_ADDR`     | `0.0.0.0:8080`           | Address and port to listen on.                                                              |
| `PUBLIC_BUCKET`   | `public`                 | Bucket served under `/public/...`.                                                          |
| `PRIVATE_BUCKET`  | `private`                | Bucket served under `/private/...`.                                                         |
| `ALLOWED_SIZES`   | `480,640,1280,1920,2560` | Comma-separated values allowed for `w`, each from 1 to 8192. Other values are rejected.     |
| `PRIVATE_MAX_AGE` | `86400`                  | Upper limit, in seconds, for `Cache-Control: max-age` on private responses.                 |
| `RUST_LOG`        | `info`                   | Log level/filter, e.g. `debug` or `img_proxy=debug`.                                        |

## URLs

```text
/public/{key}                         full size, format picked from Accept
/public/{key}?w=640&fmt=webp          scaled to width, aspect ratio kept
/public/{key}?original                source file, untouched
/private/{key}?w=640&exp={unix}&sig={sig}
```

- `w`: target width, must be one of `ALLOWED_SIZES`. Images are never upscaled.
- `fmt`: `avif`, `webp`, `jpeg` (or `jpg`) or `png`.
- `original` can't be combined with `w` or `fmt`.
- Keys may only contain `A-Z a-z 0-9 . _ -` and `/`, are at most 512 characters, and must end in `.jpg`, `.jpeg`, `.png`, `.webp`, `.avif` or `.gif`.
- Only `GET` and `HEAD` are allowed. Responses from imagor that aren't JPEG, PNG, WebP, AVIF or GIF are refused with `415`.

Invalid requests get `404` (unknown path or key), `400` (bad query), `403` (missing or wrong signature) or `410` (signature valid but expired). Private responses are sent with `Cache-Control: private, max-age=…`, capped at both `exp` and `PRIVATE_MAX_AGE`.

Private URLs are signed like this:

```text
sig = base64url_nopad(HMAC-SHA256(SIGNING_SECRET, "private/{key}\n{exp}"))
```

The signature covers only the key and the expiry, so one signature works for every size or format of the same image.
