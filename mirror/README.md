# ReachyMiniOS Hugging Face mirror

GitHub release downloads are slow for the ~1.6 GB OS image. Each release is
mirrored to the Hugging Face **Storage Bucket** `pollen-robotics/reachy-mini-os`
(xet CDN, much faster and more reliable). A bucket (not a dataset) is used
because we only need the *latest* image, not a versioned history: it is
overwritten in place on every release. The flasher pulls from there first and
falls back to GitHub.

## Where the CI lives

The mirror workflow + script live in the **`pollen-robotics/reachy-mini-os`**
repo, added via PR:

- PR: https://github.com/pollen-robotics/reachy-mini-os/pull/76
- `.github/workflows/generate_img.yml` (the `mirror-to-hf` job)
- `.github/scripts/mirror_to_hf.py`

It runs on version tags as a `mirror-to-hf` job that `needs: build-image`, so it
reuses the built artifact without rebuilding. Requires a repo secret `HF_TOKEN`
with write access to the `pollen-robotics` namespace.

## What lands on HF

The bucket is written at fixed paths (overwrite-in-place, no per-tag history):

```
reachyminios.zip     # or .img.gz / .img (whatever the build produced)
reachyminios.bmap
latest.json          # { tag, image, bmap, sha256 }
```

`sha256` is the digest of the image file; the flasher verifies the download
against it before writing to the eMMC, so a corrupt/partial upload is caught
early (a mismatch triggers a fresh re-download) instead of only at flash time.

Public bucket files are served anonymously at
`https://huggingface.co/buckets/<namespace>/<name>/resolve/<path>` (buckets are
non-versioned, so there is no revision segment).

## Flasher side

> **Note:** the flasher no longer reads from this bucket. GitHub release assets
> are served from Azure Blob storage, which honours HTTP `Range`. A single
> stream is throttled (~3 MB/s), so `src-tauri/src/images.rs` now downloads the
> image over several parallel ranged connections, which saturates the link and
> matches the CDN mirror's throughput without the extra moving part. The bucket
> is kept around only as an optional public mirror.

`src-tauri/src/images.rs` resolves images GitHub-first:

1. Newest `*.img` / `*.img.gz` / `*.zip` already in the local cache dir, else
2. Fetch the latest release from the GitHub API and download the image asset
   with `download_ranged` (parallel `Range` chunks) + the `bmap`.

Integrity is verified at flash time by the `bmap` block checksums; a corrupt or
truncated download trips `looks_like_corrupt_image`, which purges the cache and
re-downloads on the next run.
