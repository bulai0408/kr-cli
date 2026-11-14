import json,sys,datetime as dt,math
def ts(s): return dt.datetime.fromisoformat(s.replace('Z','+00:00')).timestamp()
reqs=[]; pend={}
for l in open(sys.argv[1]):
  e=json.loads(l); k=e['type']; f=e['fields']; rid=e['run_id']
  if k=='queue_start':
    r=dict(arr=ts(e['ts']),res=f['resource'],cap=f['capacity'],lock=None); reqs.append(r); pend[(rid,f['resource'],f['task'])]=r
  elif k=='machine_lock':
    if f['held_by_ancestor'] or f['shared_in_process']: continue
    c=[r for (ri,_,_),r in pend.items() if ri==rid and 'wait' not in r and r['lock'] is None]
    if c: min(c,key=lambda r:r['arr'])['lock']=ts(e['ts'])
  elif k=='slot_acquired':
    r=pend.get((rid,f['resource'],f['task']))
    if r: r['wait']=f['waited_ms']/1000; r['acq']=ts(e['ts'])
  elif k=='slot_released':
    r=pend.pop((rid,f['resource'],f['task']),None)
    if r and 'wait' in r: r['held']=f['held_ms']/1000
done=[r for r in reqs if 'held' in r]; B=[r for r in done if r['res']=='build']
print('build requests',len(B),'unfinished build',sum(1 for r in reqs if r['res']=='build' and 'held' not in r),'bridged',sum(1 for r in done if r['lock']),'bridged build',sum(1 for r in B if r['lock']))
def fifo(jobs,slow=1.0):
  # Processor sharing: while two or more jobs hold a slot, each advances at 1/slow.
  jobs=sorted(jobs,key=lambda j:j['arr']); run=[]; t=-1e18; i=0
  while i<len(jobs) or run:
    while i<len(jobs) and jobs[i]['arr']<=t and len(run)<jobs[i]['cap']:
      j=jobs[i]; j['s']=t; j['left']=j['hold']; run.append(j); i+=1
    rate=1/slow if len(run)>1 else 1.0
    nxt=[t+j['left']/rate for j in run]
    if i<len(jobs) and len(run)<jobs[i]['cap']: nxt.append(jobs[i]['arr'])
    nt=min(nxt); ended=[j for j in run if t+j['left']/rate<=nt]
    for j in run: j['left']-=(nt-t)*rate
    t=nt
    for j in ended:
      run.remove(j); j['w']=j['s']-j['arr']; j['h']=t-j['s']
  return jobs
def stats(ws,hs):
  q=sum(ws); ws=sorted(ws); n=len(ws)
  p=lambda x: ws[max(1,math.ceil(x*n))-1]
  return dict(queued_s=round(q,1),share=round(q/(q+sum(hs)),4),p50=round(p(.5),1),p90=round(p(.9),1))
print('measured',stats([r['wait'] for r in B],[r['held'] for r in B]))
L=[dict(arr=r['arr'],cap=1,hold=r['held']+(r['acq']-r['lock']),b=r['res']=='build') for r in done if r['lock']]
Q=[dict(arr=r['arr'],cap=r['cap'],hold=r['held'],b=True) for r in B if not r['lock']]
J=[j for j in fifo(L)+fifo(Q) if j['b']]
print('calibration replay',stats([j['w'] for j in J],[r['held'] for r in B]))
for slow in (1.0,1.5):
  for c in (1,2,3):
    J=fifo([dict(arr=r['arr'],cap=c,hold=r['held']) for r in B],slow)
    print('slowdown',slow,'build',c,stats([j['w'] for j in J],[j['h'] for j in J]))
