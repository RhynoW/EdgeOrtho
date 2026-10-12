"""把兩次機隊模擬的截圖（sim_fleet_shots.py --every …）組成「前後對比」動畫 GIF：左＝改善前（處理序列化），右＝現在，同一個時間點並排，
上方標題與下方顯示已拍攝／已上傳張數與機上積壓。影像是 NLSC 正射影像（授權未確認），所以**只存本機、不要公開散布**。

用法：python scripts/sim/make_fleet_gif.py --left <截圖資料夾A> --right <截圖資料夾B> --out fleet_compare.gif [--left-title …] [--right-title …] [--width 900]
"""
import argparse, json, os

from PIL import Image, ImageDraw, ImageFont

FONTS = [r"C:\Windows\Fonts\msjh.ttc", r"C:\Windows\Fonts\mingliu.ttc", "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc"]


def font(sz):
    for f in FONTS:
        if os.path.exists(f):
            try: return ImageFont.truetype(f, sz)
            except Exception: pass
    return ImageFont.load_default()


def panel(dirpath, entry, title, w, f_big, f_small):
    im = Image.open(os.path.join(dirpath, entry["file"])).convert("RGB")
    h = int(im.height * w / im.width); im = im.resize((w, h), Image.LANCZOS)
    d = ImageDraw.Draw(im, "RGBA")
    d.rectangle((0, 0, w, 40), fill=(15, 23, 42, 235)); d.text((12, 6), title, font=f_big, fill=(255, 255, 255, 255))
    back = max(entry["captured"] - entry["done"], 0)
    d.rectangle((0, h - 38, w, h), fill=(15, 23, 42, 235))
    d.text((12, h - 31), f"已拍攝 {entry['captured']:>4}　已上傳 {entry['done']:>4}　機上積壓 {back:>4}", font=f_small, fill=(255, 255, 255, 255))
    # 積壓比例條
    frac = back / max(entry["captured"], 1)
    d.rectangle((w - 260, h - 28, w - 12, h - 12), outline=(255, 255, 255, 200)); d.rectangle((w - 260, h - 28, w - 260 + int(248 * frac), h - 12), fill=(239, 68, 68, 230))
    return im


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--left", required=True); ap.add_argument("--right", required=True); ap.add_argument("--out", required=True)
    ap.add_argument("--left-title", default="改善前：影像一張一張依序處理"); ap.add_argument("--right-title", default="現在：圖磚級鎖，多張影像並行處理")
    ap.add_argument("--width", type=int, default=900); ap.add_argument("--ms", type=int, default=350)
    a = ap.parse_args()
    L = json.load(open(os.path.join(a.left, "stats.json"))); R = json.load(open(os.path.join(a.right, "stats.json")))
    n = min(len(L), len(R)); fb, fs = font(22), font(18); frames = []
    for i in range(n):
        pl, pr = panel(a.left, L[i], a.left_title, a.width, fb, fs), panel(a.right, R[i], a.right_title, a.width, fb, fs)
        canvas = Image.new("RGB", (a.width * 2 + 6, pl.height + 34), (15, 23, 42))
        canvas.paste(pl, (0, 34)); canvas.paste(pr, (a.width + 6, 34))
        d = ImageDraw.Draw(canvas); d.text((12, 4), f"22 架無人機同時回傳（真實拍攝節奏）　t = {L[i]['t']:.0f} 秒", font=fb, fill=(255, 255, 255))
        frames.append(canvas.quantize(colors=128, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE))
    frames[0].save(a.out, save_all=True, append_images=frames[1:], duration=a.ms, loop=0, optimize=True)
    last = Image.open(a.out); print(f"{n} 格、{last.size[0]}×{last.size[1]}、{os.path.getsize(a.out)/1e6:.1f} MB → {a.out}")


main()
