"""Signed distance to the rigid guard for the offline right-arm pose solver."""
from functools import lru_cache
from pathlib import Path

try:
    import numpy as np
except ModuleNotFoundError as error:
    if error.name != "numpy":
        raise
    raise SystemExit(
        "Warrior motion generation requires NumPy. Install it with "
        "python3 -m pip install -r tools/motion/requirements.txt."
    ) from None

@lru_cache(maxsize=1)
def _geometry():
    with np.load(Path(__file__).with_name("guard-cuff-sdf.npz"),allow_pickle=False) as packed:
        return {k:packed[k] for k in packed.files}

def _rotation(q):
    x,y,z,w=q
    return np.array(((1-2*(y*y+z*z),2*(x*y-w*z),2*(x*z+w*y)),
                     (2*(x*y+w*z),1-2*(x*x+z*z),2*(y*z-w*x)),
                     (2*(x*z-w*y),2*(y*z+w*x),1-2*(x*x+y*y))))

def distances(shoulder,upper,elbow,forearm,grip,blade):
    g=_geometry()
    # Sample the actual skinned cuff triangles, expressed in the rigid guard frame.
    model=(g["offset5"]@_rotation(upper).T+g["weight5"][:,None]*shoulder+
           g["offset6"]@_rotation(forearm).T+g["weight6"][:,None]*elbow)
    local=(model-grip)@_rotation(blade)
    x=(local-g["origin"])/g["step"]
    table=g["table"]
    valid=((x>=0)&(x<np.array(table.shape)-1)).all(axis=1)
    x=x[valid]
    if len(x)==0:
        return np.array([.024])
    i=x.astype(np.int32)
    u=x-i
    s=np.zeros(len(x))
    for a in (0,1):
        for b in (0,1):
            for c in (0,1):
                corner=(a,b,c)
                factor=np.prod(np.where(corner,u,1-u),axis=1)
                s+=factor*table[tuple((i+corner).T)]
    return s

def penalty(shoulder,upper,elbow,forearm,grip,blade):
    d=distances(shoulder,upper,elbow,forearm,grip,blade)
    shortage=np.maximum(.002-d,0)
    # Give the deepest sample priority and prefer margin across the cuff patch.
    return float(shortage.max()**2+4*np.mean(shortage**2))

def floor_penalty(grip,blade,wrist,hand):
    g=_geometry()
    blade_y=grip[1]+(np.einsum("ij,j->i",g["blade"],_rotation(blade)[1])).min()
    hand_y=wrist[1]+(np.einsum("ij,j->i",g["hand"],_rotation(hand)[1])).min()
    # Keep a small margin for the native renderer's interpolated pose.
    return float(max(0,.015-blade_y)**2+max(0,.009-hand_y)**2)

@lru_cache(maxsize=1)
def _defeat_track():
    import json
    return json.loads(Path(__file__).with_name("defeat-cuff-approach.json").read_text())

def defeat_approach(tick):
    return _defeat_track().get(str(min(tick,68)))


@lru_cache(maxsize=1)
def _swipe_track():
    import json
    return json.loads(Path(__file__).with_name("swipe-cuff-approach.json").read_text())

def swipe_approach(tick):
    return _swipe_track()[str(tick)]
