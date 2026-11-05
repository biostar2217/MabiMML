"""배포본 만들기 (.venv-build 의 파이썬으로 실행).
python build_release.py stage : exe 를 빌드해 ../_build/stage 에 배포 폴더를 만든다 (이 폴더의 exe 로 점검한다)
python build_release.py zip   : 점검을 통과한 배포 폴더를 ../배포 에 zip 으로 만든다
소스, 곡 파일, 사용자 설정은 넣지 않는다."""
import hashlib
import os
import shutil
import subprocess
import sys
import zipfile

ROOT = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, ROOT)
from mabimml import __version__  # noqa: E402

TOP = f"마비노기MML변환기_v{__version__}"
BASE = os.path.dirname(ROOT)
WORK = os.path.join(BASE, "_build")
STAGE = os.path.join(WORK, "stage")
OUT = os.path.join(BASE, "배포")
LICENSES = os.path.join(ROOT, "라이선스")
KINDS = {"전체": True, "가벼운판": False}

VERSION_INFO = """VSVersionInfo(
  ffi=FixedFileInfo(filevers=({v}), prodvers=({v}), mask=0x3f, flags=0x0, OS=0x40004, fileType=0x1, subtype=0x0, date=(0, 0)),
  kids=[StringFileInfo([StringTable('041204B0', [
      StringStruct('FileDescription', '마비노기 MML 변환기'),
      StringStruct('FileVersion', '{d}'),
      StringStruct('InternalName', 'MabiMML'),
      StringStruct('OriginalFilename', 'MabiMML.exe'),
      StringStruct('ProductName', '마비노기 MML 변환기'),
      StringStruct('ProductVersion', '{d}')])]),
    VarFileInfo([VarStruct('Translation', [1042, 1200])])])
"""


def build_exe():
    shutil.rmtree(WORK, ignore_errors=True)
    os.makedirs(WORK)
    nums = (__version__.split(".") + ["0"] * 4)[:4]
    info = os.path.join(WORK, "version.txt")
    with open(info, "w", encoding="utf-8") as f:
        f.write(VERSION_INFO.format(v=", ".join(nums), d=".".join(nums)))
    subprocess.run([sys.executable, "-m", "PyInstaller", "--noconfirm", "--clean", "--onedir", "--windowed",
                    "--name", "MabiMML", "--version-file", info,
                    "--icon", os.path.join(ROOT, "mabimml", "assets", "app.ico"),
                    "--add-data", os.path.join(ROOT, "mabimml", "assets") + os.pathsep + "mabimml/assets",
                    "--distpath", os.path.join(WORK, "dist"),
                    "--workpath", os.path.join(WORK, "work"), "--specpath", WORK,
                    os.path.join(ROOT, "run_gui.py")], check=True, cwd=ROOT)
    return os.path.join(WORK, "dist", "MabiMML")


def make_stage(app):
    for kind, audiveris in KINDS.items():
        top = os.path.join(STAGE, kind, TOP)
        shutil.copytree(app, top)
        shutil.copy(os.path.join(ROOT, "README.md"), top)
        shutil.copy(os.path.join(ROOT, "사용법.txt"), top)
        licenses = os.path.join(top, "라이선스")
        os.makedirs(licenses)
        shutil.copy(os.path.join(sys.base_prefix, "LICENSE.txt"), os.path.join(licenses, "Python-LICENSE.txt"))
        if audiveris:
            shutil.copytree(os.path.join(ROOT, "audiveris"), os.path.join(top, "audiveris"))
            for name in os.listdir(LICENSES):
                shutil.copy(os.path.join(LICENSES, name), licenses)


def build_portable():
    """한 파일 exe 를 ../_build/portable 에 만든다: PDF 인식 포함판."""
    work = os.path.join(BASE, "_build")
    # 분석 캐시(work 폴더)는 남겨 두어 다음 빌드를 빠르게 한다. 결과 exe 폴더만 비운다.
    shutil.rmtree(os.path.join(work, "portable"), ignore_errors=True)
    os.makedirs(work, exist_ok=True)
    nums = (__version__.split(".") + ["0"] * 4)[:4]
    info = os.path.join(work, "version.txt")
    with open(info, "w", encoding="utf-8") as f:
        f.write(VERSION_INFO.format(v=", ".join(nums), d=".".join(nums)))
    # assets 는 api_key.txt(평문 키)를 뺀 사본을 넣고, 키는 가린 형태(_k.py)로만 넣는다
    src_assets = os.path.join(ROOT, "mabimml", "assets")
    staged = os.path.join(work, "assets")
    shutil.rmtree(staged, ignore_errors=True)
    shutil.copytree(src_assets, staged, ignore=shutil.ignore_patterns("api_key.txt"))
    assets = staged + os.pathsep + "mabimml/assets"
    from mabimml import ai, keyobf
    key_module = os.path.join(ROOT, "mabimml", "_k.py")
    if os.path.exists(key_module):      # 지난 빌드가 중간에 멈춰 남은 파일은 버리고 api_key.txt 에서 새로 만든다
        os.remove(key_module)
    key = ai.builtin_key()
    if key:
        with open(key_module, "w", encoding="utf-8") as f:
            f.write(keyobf.module_text(key))
    print("내장 API 키:", "넣음" if key else "없음 (AI 명령은 사용자 PC에 저장된 키가 있어야 동작)")
    try:
        _build_portable_exes(work, info, assets)
    finally:
        if os.path.exists(key_module):
            os.remove(key_module)      # 가린 키 파일은 빌드 때만 잠깐 둔다


def _build_portable_exes(work, info, assets):
    # AI 명령용 anthropic SDK 와 의존 패키지 (vendor 폴더). 사용자 설정 파일과 평문 키는 넣지 않는다.
    vendor = ["--paths", os.path.join(ROOT, "vendor"), "--hidden-import", "anthropic",
              "--collect-submodules", "anthropic", "--collect-submodules", "pydantic",
              "--collect-submodules", "httpx2", "--collect-submodules", "httpcore2",
              "--collect-submodules", "truststore", "--copy-metadata", "anthropic"]
    # 사용자 요청(2026-11-05): 가벼운판은 더 만들지 않고 PDF 인식 포함판만 만든다
    for name, extra in ((f"MabiMML_v{__version__}_PDF",
                         ["--add-data", os.path.join(ROOT, "audiveris") + os.pathsep + "audiveris",
                          "--add-data", LICENSES + os.pathsep + "licenses"]),):
        subprocess.run([sys.executable, "-m", "PyInstaller", "--noconfirm", "--onefile", "--windowed", "--name", name,
                        "--version-file", info, "--icon", os.path.join(ROOT, "mabimml", "assets", "app.ico"),
                        "--add-data", assets, *vendor, *extra, "--distpath", os.path.join(work, "portable"),
                        "--workpath", os.path.join(work, "work", name), "--specpath", work,
                        os.path.join(ROOT, "run_gui.py")], check=True, cwd=ROOT,
                       env={**os.environ, "PYTHONPATH": os.path.join(ROOT, "vendor")})   # 패키지 정보도 찾게


def make_zip(kind):
    os.makedirs(OUT, exist_ok=True)
    path = os.path.join(OUT, f"{TOP}_{kind}.zip")
    if os.path.exists(path):
        os.remove(path)
    base = os.path.join(STAGE, kind)
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as z:
        for d, _, files in os.walk(base):
            for name in files:
                full = os.path.join(d, name)
                z.write(full, os.path.relpath(full, base).replace("\\", "/"))
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            digest.update(chunk)
    print(os.path.basename(path), os.path.getsize(path), digest.hexdigest())


def main():
    step = sys.argv[1] if len(sys.argv) > 1 else ""
    if step == "stage":
        make_stage(build_exe())
    elif step == "portable":
        build_portable()
    elif step == "zip":
        for kind in KINDS:
            make_zip(kind)
    else:
        print(__doc__)


if __name__ == "__main__":
    main()
