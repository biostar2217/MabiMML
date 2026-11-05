"""MusicXML (.musicxml / .xml / .mxl) 읽기. score-partwise 형식 지원."""
import zipfile
import xml.etree.ElementTree as ET

from .model import Note, Track, Song, UNITS_PER_QUARTER


class MusicXMLError(Exception):
    pass


_STEP = {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}
_DYN = {"pppp": 15, "ppp": 25, "pp": 36, "p": 49, "mp": 64, "mf": 80,
        "f": 96, "ff": 112, "fff": 124, "ffff": 127, "sf": 100, "sfz": 100, "fz": 100}


def _strip_ns(root):
    for el in root.iter():
        if isinstance(el.tag, str) and "}" in el.tag:
            el.tag = el.tag.split("}", 1)[1]
    return root


def _load_root(path):
    if zipfile.is_zipfile(path):
        with zipfile.ZipFile(path) as z:
            target = None
            try:
                cont = _strip_ns(ET.fromstring(z.read("META-INF/container.xml")))
                rf = cont.find(".//rootfile")
                if rf is not None:
                    target = rf.get("full-path")
            except KeyError:
                pass
            if not target:
                cands = [n for n in z.namelist()
                         if n.lower().endswith((".xml", ".musicxml")) and not n.startswith("META-INF")]
                if not cands:
                    raise MusicXMLError("압축 파일 안에 MusicXML이 없습니다")
                target = cands[0]
            return _strip_ns(ET.fromstring(z.read(target)))
    return _strip_ns(ET.parse(path).getroot())


def _num(el, path, default=None, cast=float):
    x = el.find(path)
    if x is None or x.text is None:
        return default
    try:
        return cast(x.text.strip())
    except ValueError:
        return default


def read_musicxml(path):
    root = _load_root(path)
    if root.tag == "score-timewise":
        raise MusicXMLError("score-timewise 형식은 지원하지 않습니다 (악보 프로그램에서 일반 MusicXML로 다시 내보내 주세요)")
    if root.tag != "score-partwise":
        raise MusicXMLError("MusicXML 악보가 아닙니다")

    names = {}
    for sp in root.findall("./part-list/score-part"):
        names[sp.get("id")] = (sp.findtext("part-name") or sp.get("id") or "").strip()
    title = (root.findtext("./work/work-title") or root.findtext("./movement-title") or "").strip()

    tracks, tempos, timesigs, bars = [], [], [], []
    for pi, part in enumerate(root.findall("part")):
        pid = part.get("id")
        divisions = 1.0
        pos = 0.0           # 유닛
        last_start = 0.0
        vel = 80
        notes = []
        open_ties = {}      # (pitch, voice) -> Note
        for measure in part.findall("measure"):
            if pi == 0:
                bars.append(pos)
            for el in measure:
                tag = el.tag
                if tag == "attributes":
                    d = _num(el, "divisions")
                    if d:
                        divisions = d
                    if pi == 0:
                        for tm in el.findall("time"):
                            b = _num(tm, "beats", cast=lambda s: int(s.split("+")[0]))
                            bt = _num(tm, "beat-type", cast=int)
                            if b and bt:
                                timesigs.append((pos, b, bt))
                elif tag in ("backup", "forward"):
                    dur = _num(el, "duration", 0) * UNITS_PER_QUARTER / divisions
                    pos += -dur if tag == "backup" else dur
                elif tag in ("direction", "sound"):
                    snds = [el] if tag == "sound" else el.findall(".//sound")
                    for s in snds:
                        if s.get("tempo") and pi == 0:
                            try:
                                tempos.append((pos, float(s.get("tempo"))))
                            except ValueError:
                                pass
                        if s.get("dynamics"):
                            try:
                                vel = max(1, min(127, int(float(s.get("dynamics")) * 0.9)))
                            except ValueError:
                                pass
                    if tag == "direction":
                        dyn = el.find(".//dynamics")
                        if dyn is not None and not any(s.get("dynamics") for s in snds):
                            for d in dyn:
                                if d.tag in _DYN:
                                    vel = _DYN[d.tag]
                        if pi == 0 and not any(s.get("tempo") for s in snds):
                            met = el.find(".//metronome")
                            if met is not None:
                                pm = _num(met, "per-minute")
                                unit = met.findtext("beat-unit") or "quarter"
                                mult = {"whole": 4, "half": 2, "quarter": 1, "eighth": 0.5,
                                        "16th": 0.25}.get(unit, 1)
                                if met.find("beat-unit-dot") is not None:
                                    mult *= 1.5
                                if pm:
                                    tempos.append((pos, pm * mult))
                elif tag == "note":
                    if el.find("grace") is not None or el.find("cue") is not None:
                        continue
                    dur = _num(el, "duration", 0) * UNITS_PER_QUARTER / divisions
                    is_chord = el.find("chord") is not None
                    start = last_start if is_chord else pos
                    if not is_chord:
                        last_start = pos
                        pos += dur
                    p = el.find("pitch")
                    if p is None or dur <= 0:
                        continue   # 쉼표, 타악기
                    step = (p.findtext("step") or "C").strip()
                    alter = _num(p, "alter", 0)
                    octv = _num(p, "octave", 4, int)
                    pitch = (octv + 1) * 12 + _STEP.get(step, 0) + int(round(alter))
                    voice = (el.findtext("voice") or "1").strip()
                    ties = [t.get("type") for t in el.findall("tie")]
                    ties += [t.get("type") for t in el.findall("./notations/tied")]
                    key = (pitch, voice)
                    prev = open_ties.get(key)
                    if "stop" in ties and prev is not None and abs(prev.end - start) < 1:
                        prev.end = start + dur
                        n = prev
                    else:
                        n = Note(start, start + dur, pitch, vel)
                        notes.append(n)
                    if "start" in ties:
                        open_ties[key] = n
                    else:
                        open_ties.pop(key, None)
        notes.sort(key=lambda n: (n.start, -n.pitch))
        tracks.append(Track(name=names.get(pid) or f"파트 {pi + 1}", notes=notes))

    tempos = sorted(dict(tempos).items())  # 같은 위치 중복 제거
    return Song(tracks=tracks, tempos=tempos, timesigs=sorted(timesigs), title=title, bars=bars)
