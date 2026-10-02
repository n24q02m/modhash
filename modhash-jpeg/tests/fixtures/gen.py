from PIL import Image
import os
def make_rgb(w,h,kind):
    im = Image.new("RGB",(w,h))
    px = im.load()
    for y in range(h):
        for x in range(w):
            if kind=="gradient":
                px[x,y]=((x*7+y*3)%256, (x*5+y*11)%256, (x*13+y*2)%256)
            elif kind=="blocks":
                cx,cy = x//8, y//8
                px[x,y]=((cx*37+cy*11)%256,(cx*3+cy*53)%256,(cx*97+cy*7)%256)
            else:
                px[x,y]=(((x*x+y*y)//3)%256,(x*9)%256,(y*17)%256)
    return im
def make_gray(w,h):
    im = Image.new("L",(w,h))
    px=im.load()
    for y in range(h):
        for x in range(w):
            px[x,y]=((x*9+y*13+((x//16)^(y//16))*80)%256)
    return im
os.makedirs("out",exist_ok=True)
im=make_rgb(32,24,"gradient"); im.save("out/base_444.jpg",quality=90,subsampling=0)
im=make_rgb(40,24,"blocks"); im.save("out/base_422.jpg",quality=90,subsampling=1)
im=make_rgb(48,32,"curves"); im.save("out/base_420_rst.jpg",quality=88,subsampling=2,restart_marker_blocks=2)
im=make_gray(24,40); im.save("out/base_gray.jpg",quality=92)
im=make_rgb(32,24,"gradient"); im.save("out/prog_420.jpg",quality=90,subsampling=2,progressive=True)
im=make_rgb(40,24,"blocks"); im.save("out/prog_444.jpg",quality=90,subsampling=0,progressive=True)
im=make_gray(24,40); im.save("out/prog_gray.jpg",quality=92,progressive=True)
print("saved")

# --- gen2 additions (merged) ---
from PIL import Image
import os
def make_rgb(w,h,kind):
    im = Image.new("RGB",(w,h))
    px = im.load()
    for y in range(h):
        for x in range(w):
            if kind=="gradient":
                px[x,y]=((x*7+y*3)%256, (x*5+y*11)%256, (x*13+y*2)%256)
            elif kind=="blocks":
                cx,cy = x//8, y//8
                px[x,y]=((cx*37+cy*11)%256,(cx*3+cy*53)%256,(cx*97+cy*7)%256)
            else:
                px[x,y]=(((x*x+y*y)//3)%256,(x*9)%256,(y*17)%256)
    return im
os.makedirs("out",exist_ok=True)
# odd width 4:2:2 baseline (dw=21 > out_w/2 → padded-pitch path)
im=make_rgb(41,23,"blocks"); im.save("out/base_422_odd.jpg",quality=90,subsampling=1)
# odd width AND height 4:2:0 baseline
im=make_rgb(33,17,"gradient"); im.save("out/base_420_odd.jpg",quality=90,subsampling=2)
# progressive + restart markers
im=make_rgb(32,24,"curves"); im.save("out/prog_444_rst.jpg",quality=88,subsampling=0,progressive=True,restart_marker_blocks=2)
# 4:4:0 via ffmpeg (Pillow can't encode it)
im=make_rgb(24,32,"blocks"); im.save("out/_440_src.png")
print("done")
