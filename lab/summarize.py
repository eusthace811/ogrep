import json,math,sys,glob,os
d=sys.argv[1]
print(f"{'workload':28} {'n':>4} {'ogrep/zgrep':>12} {'ogrep/tgrep':>12} {'zgrep/tgrep':>12} {'slower-vs-zgrep':>16}")
for f in sorted(glob.glob(d+'/*.json')):
    r=json.load(open(f)); s=r['summary']
    def gm(a,b):
        x=[s[q][a]['median_ms']/s[q][b]['median_ms'] for q in s]; return math.exp(sum(map(math.log,x))/len(x))
    slower=[q for q in s if s[q]['ogrep']['median_ms']>s[q]['zgrep']['median_ms']*1.02]
    print(f"{os.path.basename(f)[:-5]:28} {len(s):>4} {gm('zgrep','ogrep'):>11.3f}x {gm('tgrep','ogrep'):>11.3f}x {gm('tgrep','zgrep'):>11.3f}x {len(slower):>16}")
