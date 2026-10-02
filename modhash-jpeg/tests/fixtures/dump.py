from PIL import Image
import struct, os, glob, hashlib
def markers(path):
    d=open(path,'rb').read()
    i=2; out=[]
    while i < len(d):
        if d[i]!=0xff:
            j=i
            while j < len(d)-1:
                if d[j]==0xff and d[j+1]!=0x00 and not (0xd0<=d[j+1]<=0xd7):
                    break
                j+=1
            out.append((f'DATA {j-i}B',)); i=j; continue
        while i<len(d) and d[i]==0xff: i+=1
        m=d[i]; i+=1
        if m in (0x01,) or 0xd0<=m<=0xd9:
            out.append((f'FF{m:02X}',))
            if m==0xd9: break
            continue
        L=struct.unpack('>H',d[i:i+2])[0]
        seg=d[i+2:i+L]
        info=f'FF{m:02X} len={L}'
        if m in (0xc0,0xc1,0xc2,0xc3,0xc9,0xca,0xcb):
            P=seg[0]; Y,X=struct.unpack('>HH',seg[1:5]); N=seg[5]
            comps=[(seg[6+3*k],seg[7+3*k],seg[8+3*k]) for k in range(N)]
            info+=f' SOF P={P} {X}x{Y} comps={comps}'
        elif m==0xda:
            Ns=seg[0]; comps=[(seg[1+2*k],seg[2+2*k]) for k in range(Ns)]
            Ss,Se,AhAl=seg[1+2*Ns],seg[2+2*Ns],seg[3+2*Ns]
            info+=f' SOS comps={comps} Ss={Ss} Se={Se} Ah={AhAl>>4} Al={AhAl&15}'
        elif m==0xdb:
            p=0; tabs=[]
            while p<len(seg):
                tq=seg[p]&15; pq=seg[p]>>4
                tabs.append((pq,tq)); p+=1+64*(2 if pq else 1)
            info+=f' DQT {tabs}'
        elif m==0xc4:
            p=0; tabs=[]
            while p<len(seg):
                tc=seg[p]>>4; th=seg[p]&15
                cnt=sum(seg[p+1:p+17]); tabs.append((tc,th,cnt)); p+=17+cnt
            info+=f' DHT {tabs}'
        elif m==0xdd:
            info+=f' DRI={struct.unpack(">H",seg[:2])[0]}'
        elif 0xe0<=m<=0xef:
            info+=f' APP{m-0xe0} {seg[:12]!r}'
        out.append((info,)); i+=L
    return out
for f in sorted(glob.glob('*.jpg')):
    print('===',os.path.basename(f), os.path.getsize(f))
    for m in markers(f): print('  ',*m)
    im=Image.open(f); im.load()
    raw=im.tobytes()
    h=hashlib.sha256(raw).hexdigest()[:16]
    open(f.replace('.jpg','.raw'),'wb').write(raw)
    print(f'   mode={im.mode} size={im.size} raw={len(raw)}B sha={h}')
