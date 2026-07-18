import wave, numpy as np

def load(path):
    w = wave.open(path, 'rb')
    rate = w.getframerate()
    n = w.getnframes()
    raw = w.readframes(n)
    w.close()
    x = np.frombuffer(raw, dtype=np.int16).astype(np.float64) / 32768.0
    return rate, x

# FSK 参数（与 timestamp.rs 一致）
FSK0, FSK1 = 7000.0, 7500.0
SYM_MS = 20.0
PREAMBLE, DATA = 8, 27
GUARD_MS = 200.0

def goertzel(win, freq, rate):
    N = len(win)
    k = round(N * freq / rate)
    w = 2*np.pi*k/N
    coeff = 2*np.cos(w)
    s1 = s2 = 0.0
    for s in win:
        s0 = s + coeff*s1 - s2
        s2, s1 = s1, s0
    return s1*s1 + s2*s2 - coeff*s1*s2

def marker_len(rate):
    sym = int(SYM_MS*rate/1000)
    return (PREAMBLE+DATA)*sym + int(GUARD_MS*rate/1000)

def decode_at(x, rate, off):
    sym = int(SYM_MS*rate/1000)
    total = PREAMBLE+DATA
    if off + marker_len(rate) > len(x):
        return None
    bits=[]
    for i in range(total):
        st = off + i*sym
        win = x[st:st+sym]
        e0 = goertzel(win, FSK0, rate)
        e1 = goertzel(win, FSK1, rate)
        if max(e0,e1) < 1e-10: return None
        bit = 1 if e1>e0 else 0
        sep = abs(e1-e0)/max(e1+e0,1e-20)
        tot = np.sum(win*win)
        pur = 2*max(e0,e1)/max(sym*tot,1e-20)
        if sep<0.20 or pur<0.20: return None
        bits.append(bit)
    ham = sum(1 for i in range(PREAMBLE) if bits[i]!=(1 if i%2==0 else 0))
    if ham>1: return None
    millis = 0
    for b in bits[PREAMBLE:]:
        millis = (millis<<1)|b
    if millis>=86400000: return None
    return millis, off

def decode_with_offset(x, rate):
    m = decode_at(x, rate, 0)
    if m: return m
    sw = max(rate//1000,1)
    lim = min(len(x)-marker_len(rate), rate*2)
    # energy-based estimate
    maxe=0
    for st in range(0, lim+1, sw):
        win=x[st:st+sw]; e=np.mean(win*win); maxe=max(maxe,e)
    thr=maxe*0.08
    est=None
    for st in range(0, lim+1, sw):
        win=x[st:st+sw]
        if np.mean(win*win)>=thr:
            est=st; break
    for cand in range(max(est-sw*2,0), min(est+sw*2,lim)+1):
        m=decode_at(x,rate,cand)
        if m: return m
    sym=int(SYM_MS*rate/1000); step=max(sym//4,1)
    for cand in range(0,lim+1,step):
        m=decode_at(x,rate,cand)
        if m: return m
    return None

def fmt(ms):
    ms=int(round(ms)); h=(ms//3600000)%24; mn=(ms//60000)%60; s=(ms//1000)%60; mss=ms%1000
    return f"{h:02d}:{mn:02d}:{s:02d}.{mss:03d}"

# 简单能量包络找拨弦起音（文件内相对位置）
def find_onsets(x, rate, start, min_gap_s=2.0):
    seg = x[start:]
    hop=16; win=160
    frames=[]
    for i in range(0,len(seg)-win,hop):
        frames.append(np.sqrt(np.mean(seg[i:i+win]**2)))
    frames=np.array(frames)
    base=np.percentile(frames,20)
    peak=frames.max()
    thr=base+(peak-base)*0.15
    onsets=[]
    active=False; astart=0; quiet=0
    for idx,v in enumerate(frames):
        if v>thr:
            if not active: active=True; astart=idx
            quiet=0
        elif active:
            quiet+=1
            if quiet>=25:
                onsets.append(astart); active=False
    if active: onsets.append(astart)
    # 合并 min_gap
    merged=[]
    gapf=int(min_gap_s*rate/hop)
    for o in onsets:
        if merged and o<merged[-1]+gapf: continue
        merged.append(o)
    # 转成样本（相对 start），并精定位起音（12%阈值）
    result=[]
    for o in merged:
        s0=max(o-5,0)
        local=frames[s0:o+30]
        lb=local.min(); lp=local.max(); t=lb+(lp-lb)*0.12
        pos=o
        for j in range(s0,o+30):
            if frames[j]>=t:
                pos=j; break
        result.append(pos*hop)  # 相对 actual_audio 起点的样本
    return result

for f in ["recordinga.wav","recordingb.wav"]:
    rate,x = load(f)
    dec = decode_with_offset(x, rate)
    if not dec:
        print(f, "FSK decode FAIL"); continue
    millis, off = dec
    mlen = marker_len(rate)
    marker_end = off+mlen
    print("="*60)
    print(f"{f}: rate={rate} dur={len(x)/rate:.3f}s")
    print(f"  FSK起始偏移(off)= {off}样本 = {off/rate*1000:.1f}ms")
    print(f"  FSK开始时间(millis_of_day) = {fmt(millis)}  ({millis}ms)")
    print(f"  标记总长 marker_len = {mlen}样本 = {mlen/rate*1000:.1f}ms")
    print(f"  实际录音区起点(marker_end) = {marker_end}样本 = {marker_end/rate*1000:.1f}ms")
    onsets = find_onsets(x, rate, marker_end)
    print(f"  检测到事件数: {len(onsets)}")
    for i,o in enumerate(onsets):
        # 事件绝对时间 = millis + marker_end样本*1000/rate + o*1000/rate
        # 注意: analyze.rs 用 (marker_offset+marker_samples), 即 marker_end
        abs_ms = millis + marker_end*1000/rate + o*1000/rate
        rel_ms = o*1000/rate  # 相对实际录音区起点
        print(f"    事件{i+1}: 录音区内偏移={rel_ms:8.1f}ms  绝对={fmt(abs_ms)}")
