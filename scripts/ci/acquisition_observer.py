"""Bounded, nonmutating observation of an initially absent owned native cache.

This establishes local assembly only. It never qualifies a guest or a live proof.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import struct
import zipfile

MAX_DIRECTORIES = 16
MAX_MANIFEST_BYTES = 1024 * 1024
MAX_BRIDGE_BYTES = 64 * 1024 * 1024
MAX_ARCHIVE_BYTES = 512 * 1024 * 1024
MAX_ENTRIES = 4096
MAX_CENTRAL_BYTES = 1024 * 1024
MAX_EXPANDED_BYTES = 2 * 1024 * 1024 * 1024
HASH = re.compile(r'[a-f0-9]{64}\Z')
BUNDLE = re.compile(r'cua-driver-rs-v0\.34\.0-[a-f0-9]{16}\Z')
ARCHIVE = 'cua-driver-rs-0.34.0-windows-x86_64-binary.zip'


class BudgetExceeded(Exception):
    pass


def plain(path, directory=False):
    value=path.lstat()
    if (value.st_file_attributes & 0x400 if hasattr(value,'st_file_attributes') else False):
        raise ValueError('Reparse file')
    if not (stat.S_ISDIR(value.st_mode) if directory else stat.S_ISREG(value.st_mode)):
        raise ValueError('Special file')
    return value


def ancestry(path):
    for parent in reversed(path.parents):plain(parent,True)
    plain(path,True)


def entries(path, limit):
    ancestry(path)
    found=[]
    with os.scandir(path) as scan:
        for entry in scan:
            if len(found)>=limit:raise BudgetExceeded()
            found.append(Path(entry.path))
    return found


def open_plain(path, limit):
    ancestry(path.parent)
    before=plain(path)
    if before.st_size>limit:raise BudgetExceeded()
    descriptor=os.open(path,os.O_RDONLY|getattr(os,'O_BINARY',0)|getattr(os,'O_NOFOLLOW',0))
    try:
        opened=os.fstat(descriptor)
        after=plain(path)
        ancestry(path.parent)
        if ((before.st_dev,before.st_ino,before.st_size)!=(opened.st_dev,opened.st_ino,opened.st_size)
            or (after.st_dev,after.st_ino)!=(opened.st_dev,opened.st_ino)):
            raise ValueError('File identity changed')
        file=os.fdopen(descriptor,'rb');descriptor=None
        return file
    finally:
        if descriptor is not None:os.close(descriptor)


def digest(stream, budget):
    hashed=hashlib.sha256();total=0
    while block:=stream.read(min(1024*1024,budget-total+1)):
        total+=len(block)
        if total>budget:raise BudgetExceeded()
        hashed.update(block)
    return hashed.hexdigest(),total


def name_valid(name):
    return (isinstance(name,str) and 0<len(name.encode('utf-8'))<=255 and name not in ('.','..')
            and not name.startswith(' ') and not name.endswith(('.', ' '))
            and not any(ord(c)<32 or 127<=ord(c)<=159 or c in '/\\:*?"<>|' for c in name))


def unique_json(items):
    found={}
    for key,value in items:
        if key in found:raise ValueError('Duplicate key')
        found[key]=value
    return found


def admit_zip(file):
    """Bound metadata allocation before ZipFile constructs any ZipInfo objects."""
    size=file.seek(0,os.SEEK_END)
    start=max(0,size-65535-22-20)
    file.seek(start);tail=file.read(size-start)
    offset=tail.rfind(b'PK\x05\x06')
    if offset<0 or len(tail)-offset<22:raise ValueError('ZIP end record')
    disk,central_disk,on_disk,total,central_size,central_offset,comment=struct.unpack_from('<4H2IH',tail,offset+4)
    if offset+22+comment!=len(tail):raise ValueError('ZIP end record shape')
    if (total==65535 or on_disk==65535 or central_size==2**32-1 or central_offset==2**32-1
        or tail[max(0,offset-20):offset].startswith(b'PK\x06\x07')):
        raise BudgetExceeded('ZIP64 is outside the observer budget')
    if total>MAX_ENTRIES or central_size>MAX_CENTRAL_BYTES:raise BudgetExceeded()
    if disk or central_disk or total!=on_disk or central_offset+central_size!=start+offset:
        raise ValueError('ZIP central directory boundaries')
    file.seek(central_offset);central=file.read(central_size)
    if len(central)!=central_size:raise ValueError('ZIP central directory truncated')
    cursor=count=0
    while cursor<len(central):
        if count>=MAX_ENTRIES:raise BudgetExceeded()
        if cursor+46>len(central) or central[cursor:cursor+4]!=b'PK\x01\x02':raise ValueError('ZIP central entry')
        if any(struct.unpack_from('<I',central,cursor+pos)[0]==2**32-1 for pos in (20,24,42)):
            raise BudgetExceeded('ZIP64 entry')
        name,extra,entry_comment=struct.unpack_from('<3H',central,cursor+28)
        cursor+=46+name+extra+entry_comment;count+=1
    if cursor!=len(central) or count!=total:raise ValueError('ZIP central entry count')
    file.seek(0)


def verify_entry(path, expected_hash):
    names=entries(path,3)
    if {p.name for p in names}!={'manifest.json','rdpilot-bridge.exe',ARCHIVE}:raise ValueError('Bundle contents')
    with open_plain(path/'manifest.json',MAX_MANIFEST_BYTES) as file:
        data=file.read(MAX_MANIFEST_BYTES+1)
        if len(data)>MAX_MANIFEST_BYTES:raise BudgetExceeded()
        manifest=json.loads(data,object_pairs_hook=unique_json)
    if not isinstance(manifest,dict) or set(manifest)!={'bundle_id','cua_version','archive_name','archive_sha256','bridge_sha256','files'}:
        raise ValueError('Manifest schema')
    if (manifest['bundle_id']!=path.name or manifest['cua_version']!='0.34.0' or manifest['archive_name']!=ARCHIVE
        or manifest['bridge_sha256']!=expected_hash or not HASH.fullmatch(str(manifest['archive_sha256']))):
        raise ValueError('Manifest identity')
    computed_id='cua-driver-rs-v0.34.0-'+hashlib.sha256((expected_hash+manifest['archive_sha256']).encode('ascii')).hexdigest()[:16]
    if path.name!=computed_id:raise ValueError('Bundle ID')
    files=manifest['files']
    if not isinstance(files,dict) or not files or 'cua-driver.exe' not in files:raise ValueError('File table')
    if len(files)>MAX_ENTRIES:raise BudgetExceeded()
    if (len({key.lower() for key in files})!=len(files)
        or any(not name_valid(key) or key.lower() in ('manifest.json','rdpilot-bridge.exe')
               or not isinstance(value,str) or not HASH.fullmatch(value) for key,value in files.items())):
        raise ValueError('File table names/hashes')
    with open_plain(path/'rdpilot-bridge.exe',MAX_BRIDGE_BYTES) as file:
        if digest(file,MAX_BRIDGE_BYTES)[0]!=expected_hash:raise ValueError('Source bridge hash')
    with open_plain(path/ARCHIVE,MAX_ARCHIVE_BYTES) as file:
        if digest(file,MAX_ARCHIVE_BYTES)[0]!=manifest['archive_sha256']:raise ValueError('Archive hash')
        admit_zip(file)
        with zipfile.ZipFile(file) as archive:
            members=archive.infolist()
            if len(members)>MAX_ENTRIES:raise BudgetExceeded()
            if sum(item.file_size for item in members)>MAX_EXPANDED_BYTES:raise BudgetExceeded()
            observed={};lower_names=set();total=0
            for item in members:
                mode=(item.external_attr>>16)&0xffff
                if (item.is_dir() or not name_valid(item.filename) or item.flag_bits&1
                    or (stat.S_IFMT(mode) not in (0,stat.S_IFREG))
                    or item.filename.lower() in lower_names
                    or (not item.flag_bits&0x800 and not item.filename.isascii())):
                    raise ValueError('Archive layout')
                with archive.open(item) as entry:
                    hashed,size=digest(entry,MAX_EXPANDED_BYTES-total)
                total+=size
                if size!=item.file_size:raise ValueError('Archive size')
                observed[item.filename]=hashed
                lower_names.add(item.filename.lower())
            if observed!=files:raise ValueError('Archive contents differ from manifest')
    return {'bridge_sha256':expected_hash,'archive_sha256':manifest['archive_sha256'],
            'cua_sha256':observed['cua-driver.exe'],'cua_version':'0.34.0','bundle_id':computed_id}


def observe_cache(cache, expected_hash):
    """Caller supplies only the cache root refused-if-present by preflight."""
    try:
        if not isinstance(expected_hash,str) or not HASH.fullmatch(expected_hash):raise ValueError('Source hash')
        root=Path(os.path.abspath(cache))
        ancestry(root.parent)
        if not root.exists():
            # A dangling symlink is an unsafe entry, not an absent cache.
            if root.is_symlink():raise ValueError('Cache symlink')
            return {'state':'absent','bundle_count':0}
        ancestry(root)
        bundles=root/'bundles'
        if not bundles.exists():
            if bundles.is_symlink():raise ValueError('Bundles symlink')
            return {'state':'absent','bundle_count':0}
        found=entries(bundles,MAX_DIRECTORIES)
        if not found:return {'state':'absent','bundle_count':0}
        if len(found)!=1 or not BUNDLE.fullmatch(found[0].name):raise ValueError('Ambiguous bundle')
        ancestry(found[0])
        identity=verify_entry(found[0],expected_hash)
        return {'state':'verified_source_bundle','bundle_count':1,**identity}
    except (BudgetExceeded,OSError):return {'state':'observation_failed'}
    except (ValueError,UnicodeError,TypeError,KeyError,RecursionError,zipfile.BadZipFile,RuntimeError):return {'state':'invalid'}
