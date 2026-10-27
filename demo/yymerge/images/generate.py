"""Original image comparison examples; regenerate with Pillow (not needed at runtime)."""
from pathlib import Path
from PIL import Image,ImageDraw
root=Path(__file__).resolve().parent
for side in ['left','right']:
 im=Image.new('RGBA',(320,200),(0,0,0,0))
 d=ImageDraw.Draw(im)
 d.rectangle((20,25,135,145),fill=(35,100,190,255) if side=='left' else (20,135,190,255))
 shift=0 if side=='left' else 6
 d.ellipse((180+shift,45,265+shift,130),fill=(255,160,20,180))
 d.line((30,175,285,155),fill=(60,60,60,255),width=3)
 if side=='right': d.rectangle((280,20,295,35),fill=(220,30,50,255))
 im.save(root/f'{side}.png')
 im.save(root/f'{side}.webp',lossless=True,exact=True)
 if side=='left':
  opaque=Image.new('RGB',im.size,'white');opaque.paste(im,mask=im.getchannel('A'))
  opaque.save(root/'sample.jpg',quality=95)
  opaque.save(root/'sample.bmp')
  opaque.save(root/'sample-lossy.webp',quality=85)

# Unsupported multi-frame input and supported EXIF orientation for regression checks.
a=Image.new('RGBA',(2,3),'red')
b=Image.new('RGBA',(2,3),'blue')
a.save(root/'animated.png',save_all=True,append_images=[b],duration=100,loop=0)
a.save(root/'animated.webp',save_all=True,append_images=[b],duration=100,loop=0,lossless=True)
grey=Image.new('I;16',(2,2),65535)
grey.save(root/'sixteen-bit.png')
photo=Image.new('RGB',(2,3),'red')
exif=Image.Exif();exif[274]=6
photo.save(root/'oriented.jpg',exif=exif)
