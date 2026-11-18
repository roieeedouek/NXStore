# Recover brotli-compressed Tauri v2 embedded assets from a stripped x86-64 PIE binary.
# Finds each asset key string, follows R_X86_64_RELATIVE relocations to the phf map
# entry (key ptr,len,value ptr,len), and decompresses the value.
# Usage: python3 -I tools/extract_tauri_assets.py <binary> <outdir>
# Requires: pip install brotli pyelftools
import sys, struct, brotli, os
from elftools.elf.elffile import ELFFile
from elftools.elf.relocation import RelocationSection
path, outdir = sys.argv[1], sys.argv[2]
raw=open(path,'rb').read(); e=ELFFile(open(path,'rb'))
def v2o(va):
    for s in e.iter_segments():
        if s['p_type']=='PT_LOAD' and s['p_vaddr']<=va<s['p_vaddr']+s['p_filesz']:
            return va-s['p_vaddr']+s['p_offset']
relo={}
for s in e.iter_sections():
    if isinstance(s,RelocationSection):
        for r in s.iter_relocations():
            if r['r_info_type']==8: relo[r['r_offset']]=r['r_addend']  # R_X86_64_RELATIVE
by_addend={}
for off,add in relo.items(): by_addend.setdefault(add,[]).append(off)
names=[b'/index.html']
ro=e.get_section_by_name('.rodata'); robase=ro['sh_addr']; rodata=ro.data()
os.makedirs(outdir,exist_ok=True)
for n in names:
    i=-1
    while True:
        i=rodata.find(n,i+1)
        if i<0: break
        va=robase+i
        for slot in by_addend.get(va,[]):
            klen=struct.unpack_from('<Q',raw,v2o(slot+8))[0]
            if klen!=len(n): continue
            vptr=relo.get(slot+16); vlen=struct.unpack_from('<Q',raw,v2o(slot+24))[0]
            if vptr is None: continue
            blob=raw[v2o(vptr):v2o(vptr)+vlen]
            try: data=brotli.decompress(blob)
            except Exception as ex: print(n,'brotli fail',ex); continue
            fn=os.path.join(outdir,n.decode().lstrip('/'))
            open(fn,'wb').write(data); print(n.decode(), vlen,'->',len(data))
