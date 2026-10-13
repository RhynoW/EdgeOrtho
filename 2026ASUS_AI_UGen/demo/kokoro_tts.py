"""EdgeOrtho: neural narration with Kokoro-82M (Apache-2.0) through kokoro-onnx. Runs offline once the model files are downloaded.

Model files (about 120 MB), from https://github.com/thewh1teagle/kokoro-onnx/releases/tag/model-files-v1.0 :
  kokoro-v1.0.int8.onnx  (or kokoro-v1.0.onnx)   and   voices-v1.0.bin
Install:  pip install kokoro-onnx soundfile        (a separate virtual environment is fine)
Usage:    python kokoro_tts.py --models <dir> --texts texts.json --out <dir> [--voice af_heart] [--speed 1.0]
texts.json is a list of strings; writes n0.wav, n1.wav, ... (mono, 24 kHz).
"""
import argparse, json, os, sys
import soundfile as sf
from kokoro_onnx import Kokoro


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--models", required=True); ap.add_argument("--texts", required=True); ap.add_argument("--out", required=True)
    ap.add_argument("--voice", default="af_heart"); ap.add_argument("--speed", type=float, default=1.0)
    a = ap.parse_args()
    model = next((os.path.join(a.models, f) for f in ("kokoro-v1.0.int8.onnx", "kokoro-v1.0.onnx") if os.path.exists(os.path.join(a.models, f))), None)
    if not model: sys.exit("model file not found in " + a.models)
    k = Kokoro(model, os.path.join(a.models, "voices-v1.0.bin"))
    texts = json.load(open(a.texts, encoding="utf8"))
    os.makedirs(a.out, exist_ok=True)
    for i, t in enumerate(texts):
        samples, sr = k.create(t, voice=a.voice, speed=a.speed, lang="en-us")
        sf.write(os.path.join(a.out, f"n{i}.wav"), samples, sr)
        print(f"n{i}.wav {len(samples)/sr:.1f}s", flush=True)


if __name__ == "__main__":
    main()
