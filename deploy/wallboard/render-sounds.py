"""Reproduce the wallboard's original, synthetic wood-and-air cue assets."""
from pathlib import Path
import math, random, wave, struct
RATE=48000
out=Path(__file__).resolve().parent/'sounds'; out.mkdir(parents=True,exist_ok=True)
def smooth(x):
    x=max(0,min(1,x)); return x*x*(3-2*x)
def envelope(t, length, attack):
    return smooth(t/attack)*(1-smooth((t-length+.11)/.11)) if 0<=t<length else 0

def render(kind):
    duration={'arrival':.95,'edit':.68,'departure':1.0}[kind]
    rng=random.Random({'arrival':831,'edit':832,'departure':833}[kind])
    # Damped inharmonic modes evoke wood; a filtered breath joins the gesture.
    voices={
        'arrival':[(.14,392,.25,1),(.27,523.25,.27,.58)],
        'edit':[(.045,587.33,.16,.60),(.085,783.99,.12,.15)],
        'departure':[(.06,349.23,.25,.60),(.18,261.63,.30,.75)],
    }[kind]
    samples=[]; low=0; slow=0
    for i in range(round(duration*RATE)):
        t=i/RATE
        low += .085*(rng.uniform(-1,1)-low)
        slow += .012*(low-slow)
        # The air has a rounded onset and no hiss left behind after the gesture.
        if kind=='arrival':
            breath=math.sin(math.pi*min(1,t/.66))**2 if t<.66 else 0
        elif kind=='edit':
            breath=math.sin(math.pi*min(1,t/.36))**2 if t<.36 else 0
        else:
            breath=smooth(t/.06)*math.exp(-t/ .20)
        value=(low-slow)*breath*.50
        for start,frequency,decay,strength in voices:
            age=t-start
            if age<0: continue
            strike=smooth(age/.009)*math.exp(-age/decay)*envelope(age,duration-start,.009)
            modes=sum(level*math.sin(2*math.pi*frequency*ratio*age)*math.exp(-age/decay*extra)
                      for ratio,level,extra in [(1,1,0),(2.76,.20,1.5),(5.40,.045,5)])
            # A tiny dry contact texture gives the onset a tactile edge.
            contact=(low-slow)*math.exp(-age/.018)*.22
            value += strength*strike*(modes+contact)
        value *= envelope(t,duration,.012)
        samples.append(value)
    target={'arrival':.11,'edit':.075,'departure':.10}[kind]
    scale=target/max(map(abs,samples))
    return [v*scale for v in samples]
def save(name,samples):
    assert all(math.isfinite(v) and abs(v)<.2 for v in samples)
    assert abs(samples[0])<1e-9 and abs(samples[-1])<1e-7
    with wave.open(str(out/(name+'.wav')),'wb') as f:
        f.setparams((1,2,RATE,len(samples),'NONE','not compressed'))
        f.writeframes(b''.join(struct.pack('<h',round(v*32767)) for v in samples))
    return {'seconds':len(samples)/RATE,'peak':max(map(abs,samples)),
            'rms':math.sqrt(sum(v*v for v in samples)/len(samples)),
            'largest_sample_step':max(abs(b-a) for a,b in zip(samples,samples[1:]))}
for kind in ['arrival','edit','departure']:
    print(kind, save(kind,render(kind)))
