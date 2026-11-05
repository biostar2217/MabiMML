# 파이썬판 ↔ Rust판 결과 비교

- 비교한 파이썬판: `D:\MML\마비노기MML변환기`
- 입력 18개 × 옵션 12가지 = **216번**, 바이트까지 같음 **216번**
- 비교 대상: 결과 텍스트(-o), 악보별 저장 파일(--split-dir), 미리듣기 MIDI(--preview), 종료 코드

| 입력 파일 | 기본 | F랭크·노래포함 | 32격자·셋잇단끔·볼륨2 | 자동세밀도 | 자브키엘끔·5랭크 | 템포모든파트·v0·베이스페달 | 피아노·-3반음·120% | 앞부분만·C랭크 | 고정볼륨·파트음량·정렬끔·빈틈 | 편곡·무제한 | 두사람·셋잇단항상·BPM90 | 페달끔·익스프레션끔·16격자 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 9846-la-campanella-kibee-v1.mid | ✅ 4장 | ✅ 29장 | ✅ 4장 | ✅ 4장 | ✅ 5장 | ✅ 4장 | ✅ 4장 | ✅ 1장 | ✅ 4장 | ✅ 1장 | ✅ 5장 | ✅ 3장 |
| emil-huisaeng.mid | ✅ 1장 | ✅ 10장 | ✅ 1장 | ✅ 1장 | ✅ 2장 | ✅ 1장 | ✅ 2장 | ✅ 1장 | ✅ 1장 | ✅ 1장 | ✅ 1장 | ✅ 1장 |
| golden-hour-jvke-eobdeiteu-beojeon.mid | ✅ 2장 | ✅ 21장 | ✅ 2장 | ✅ 2장 | ✅ 3장 | ✅ 3장 | ✅ 3장 | ✅ 1장 | ✅ 2장 | ✅ 1장 | ✅ 2장 | ✅ 3장 |
| janhoghan-cheonsaui-teje-sinsegi-ebangelion.mid | ✅ 3장 | ✅ 28장 | ✅ 3장 | ✅ 3장 | ✅ 4장 | ✅ 3장 | ✅ 3장 | ✅ 1장 | ✅ 3장 | ✅ 1장 | ✅ 3장 | ✅ 3장 |
| js-baheu-goldeubeleukeu-byeonjugog-bwv-988-byeonju-1beon.mid | ✅ 1장 | ✅ 9장 | ✅ 1장 | ✅ 1장 | ✅ 2장 | ✅ 1장 | ✅ 2장 | ✅ 1장 | ✅ 1장 | ✅ 1장 | ✅ 1장 | ✅ 1장 |
| komm-susser-tod-the-end-of-evangelion.mid | ✅ 2장 | ✅ 18장 | ✅ 2장 | ✅ 2장 | ✅ 3장 | ✅ 2장 | ✅ 2장 | ✅ 1장 | ✅ 2장 | ✅ 1장 | ✅ 2장 | ✅ 2장 |
| liseuteu-chojeolgigyo-yeonseubgog-10beon-badanjo-apasionata-s-139.mid | ✅ 5장 | ✅ 54장 | ✅ 5장 | ✅ 5장 | ✅ 7장 | ✅ 5장 | ✅ 5장 | ✅ 1장 | ✅ 5장 | ✅ 1장 | ✅ 4장 | ✅ 4장 |
| liseuteu-chojeolgigyo-yeonseubgog-5beon-dokkaebibul.mid | ✅ 4장 | ✅ 45장 | ✅ 4장 | ✅ 4장 | ✅ 6장 | ✅ 4장 | ✅ 4장 | ✅ 1장 | ✅ 4장 | ✅ 1장 | ✅ 5장 | ✅ 3장 |
| liseuteu-la-kampanella-yeonseubgog-s-1413-ollim-sadanjo.mid | ✅ 5장 | ✅ 42장 | ✅ 5장 | ✅ 5장 | ✅ 7장 | ✅ 5장 | ✅ 5장 | ✅ 1장 | ✅ 5장 | ✅ 1장 | ✅ 5장 | ✅ 4장 |
| liseuteu-paganini-daeyeonseubgog-s-141-je6beon-jujewa-byeonju-gadanjo.mid | ✅ 4장 | ✅ 40장 | ✅ 4장 | ✅ 4장 | ✅ 6장 | ✅ 5장 | ✅ 5장 | ✅ 1장 | ✅ 4장 | ✅ 1장 | ✅ 4장 | ✅ 4장 |
| nieo-halmeoni-piano-sollo.mid | ✅ 2장 | ✅ 16장 | ✅ 2장 | ✅ 1장 | ✅ 3장 | ✅ 2장 | ✅ 2장 | ✅ 1장 | ✅ 2장 | ✅ 1장 | ✅ 1장 | ✅ 1장 |
| pupa-deemo-phyxinon-pul-agbo.mid | ✅ 2장 | ✅ 19장 | ✅ 2장 | ✅ 2장 | ✅ 3장 | ✅ 2장 | ✅ 2장 | ✅ 1장 | ✅ 2장 | ✅ 1장 | ✅ 2장 | ✅ 2장 |
| deemo-pupa_인식.mxl | ✅ 2장 | ✅ 17장 | ✅ 2장 | ✅ 2장 | ✅ 3장 | ✅ 2장 | ✅ 2장 | ✅ 1장 | ✅ 2장 | ✅ 1장 | ✅ 2장 | ✅ 2장 |
| fly-me-to-the-moon-evangelion-ed_인식.mxl | ✅ 2장 | ✅ 14장 | ✅ 2장 | ✅ 2장 | ✅ 2장 | ✅ 2장 | ✅ 2장 | ✅ 1장 | ✅ 2장 | ✅ 1장 | ✅ 2장 | ✅ 2장 |
| fly-me-to-the-moon-evangelion-ed_인식_백업1.mxl | ✅ 2장 | ✅ 14장 | ✅ 2장 | ✅ 2장 | ✅ 2장 | ✅ 2장 | ✅ 2장 | ✅ 1장 | ✅ 2장 | ✅ 1장 | ✅ 2장 | ✅ 2장 |
| 9846-la-campanella-kibee-v1.mml | ✅ (오류 1) | ✅ (오류 1) | ✅ (오류 1) | ✅ (오류 1) | ✅ (오류 1) | ✅ (오류 1) | ✅ (오류 1) | ✅ (오류 1) | ✅ (오류 1) | ✅ (오류 1) | ✅ (오류 1) | ✅ (오류 1) |
| sample.musicxml | ✅ 1장 | ✅ 1장 | ✅ 1장 | ✅ 1장 | ✅ 1장 | ✅ 1장 | ✅ 1장 | ✅ 1장 | ✅ 1장 | ✅ 1장 | ✅ 1장 | ✅ 1장 |
| 시험MML_합침.txt | ✅ 2장 | ✅ 15장 | ✅ 2장 | ✅ 2장 | ✅ 3장 | ✅ 2장 | ✅ 2장 | ✅ 1장 | ✅ 2장 | ✅ 1장 | ✅ 2장 | ✅ 2장 |

전체 실행 시간: 파이썬판 167초, Rust판 13초

