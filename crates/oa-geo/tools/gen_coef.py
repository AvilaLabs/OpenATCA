# Regenerates src/coef.rs: series coefficients for the geodesic integrals (Karney 2013), derived with exact rational arithmetic.
# Usage: python3 tools/gen_coef.py > /dev/null   (writes coef.rs in the current directory)
from fractions import Fraction as F
from math import factorial
ORD=6
# bivariate poly in (e,n): dict {(i,j):F}, truncated total degree <= ORD
def padd(a,b,s=1):
    r=dict(a)
    for k,v in b.items():
        r[k]=r.get(k,0)+s*v
        if r[k]==0: del r[k]
    return r
def pmul(a,b):
    r={}
    for (i,j),v in a.items():
        for (k,l),w in b.items():
            if i+j+k+l<=ORD:
                key=(i+k,j+l); r[key]=r.get(key,0)+v*w
    return {k:v for k,v in r.items() if v!=0}
def pscale(a,c): return {k:v*c for k,v in a.items() if v*c!=0}
def pconst(c): return {(0,0):F(c)} if c!=0 else {}
E={(1,0):F(1)}; N={(0,1):F(1)}
# Laurent in z: dict k -> poly
def ladd(a,b,s=1):
    r={k:dict(v) for k,v in a.items()}
    for k,v in b.items():
        r[k]=padd(r.get(k,{}),v,s)
        if not r[k]: del r[k]
    return r
def lmul(a,b):
    r={}
    for k,v in a.items():
        for l,w in b.items():
            p=pmul(v,w)
            if p: r[k+l]=padd(r.get(k+l,{}),p)
    return {k:v for k,v in r.items() if v}
def lscale(a,c): return {k:pscale(v,c) for k,v in a.items()}
def lpoly(p): return {0:p} if p else {}
ONE=lpoly(pconst(1))
def lpow_series(u,coeffs):
    # sum coeffs[m]*u^m
    res={}; term=ONE
    for m,c in enumerate(coeffs):
        res=ladd(res,lscale(term,c)); term=lmul(term,u)
    return res
def binom(a,m):
    r=F(1)
    for i in range(m): r*= (a-i)/F(i+1)
    return r
# k2 = 4e/(1-e)^2 = 4 e sum (m+1) e^m
k2={}
for m in range(0,ORD):
    k2[(m+1,0)]=F(4*(m+1))
x={1:{(0,0):F(1,2)},-1:{(0,0):F(1,2)}}
oneminusx=ladd(ONE,x,-1)
u=lmul(lpoly(k2),lscale(oneminusx,F(1,2)))
def sqrt_ser(alpha): return lpow_series(u,[binom(F(alpha),m) for m in range(ORD+1)])
def fourier(L):
    a0=L.get(0,{}); al=[lscale({0:L.get(l,{})},2)[0] if L.get(l) else {} for l in range(1,ORD+1)]
    return a0,al
def pinv1(a):  # 1/(1+d)
    d=padd(a,pconst(1),-1); res=pconst(1); term=pconst(1)
    for m in range(1,ORD+1):
        term=pmul(term,pscale(d,-1)); res=padd(res,term)
    return res
S1=sqrt_ser(F(1,2)); a0,al=fourier(S1)
A1n={(0,0):F(1),(2,0):F(1,4),(4,0):F(1,64),(6,0):F(1,256)}
# check a0*(1-e)==A1n
chk=pmul(a0,padd(pconst(1),E,-1)); assert chk==A1n,(chk,A1n)
iA1=pinv1(a0)  # not used; use A1 exact
def pseries_inv(num): # 1/ (num/(1-e)) = (1-e)/num
    return pmul(padd(pconst(1),E,-1),pinv1(num))
iA1=pseries_inv(A1n)
C1=[pmul(pscale(al[l-1],F(1,2*l)),iA1) for l in range(1,ORD+1)]
S2=sqrt_ser(F(-1,2)); b0,bl=fourier(S2)
A2n=pmul(b0,padd(pconst(1),E))
print("A2n",A2n)
iA2=pmul(padd(pconst(1),E),pinv1(A2n))
C2=[pmul(pscale(bl[l-1],F(1,2*l)),iA2) for l in range(1,ORD+1)]
# C1' via D = -sum C1l [S_l cosh(2l D) + C_l sinh(2l D)]
def Sl(l): return {l:pconst(F(1,2)),-l:pconst(F(-1,2))}
def Cl(l): return {l:pconst(F(1,2)),-l:pconst(F(1,2))}
D={}
for it in range(ORD+1):
    new={}
    for l in range(1,ORD+1):
        cosh=lpow_series(lmul(D,D) if D else {},[F(1)/factorial(2*k)*(2*l)**(2*k) for k in range(ORD//2+1)])
        # sinh(y)= y * sum y^(2k)/(2k+1)!
        sinh=lmul(D,lpow_series(lmul(D,D) if D else {},[F(1)/factorial(2*k+1)*(2*l)**(2*k+1) for k in range(ORD//2+1)])) if D else {}
        term=ladd(lmul(Sl(l),cosh),lmul(Cl(l),sinh))
        term=lmul(lpoly(C1[l-1]),term)
        new=ladd(new,term,-1)
    D=new
C1p=[pscale(D.get(m,{}),2) for m in range(1,ORD+1)]
# I3
dl=ladd(S1,ONE,-1)
base=lscale(lmul(lpoly(padd(pconst(1),N,-1)),dl),F(-1,2))
hh=lpow_series(base,[F(1)]*(ORD+1))
h0,hl=fourier(hh)
iA3=pinv1(h0)
C3=[pmul(pscale(hl[l-1],F(1,2*l)),iA3) for l in range(1,ORD+1)]
A3=h0
def fr(c):
    c=F(c)
    return f"{float(c.numerator)!r}" if c.denominator==1 else f"{float(c.numerator)!r} / {float(c.denominator)!r}"
def e_slice(p):
    assert all(j==0 for (i,j) in p)
    return "&["+", ".join(fr(p.get((i,0),0)) for i in range(ORD+1))+"]"
def n_slice(p,i,deg):
    return "&["+", ".join(fr(p.get((i,j),0)) for j in range(deg+1))+"]"
out=["// Generated coefficient tables, derived symbolically from the integrals in Karney (2013);",
"// every series is truncated at total order 6 in the parameter eps (and n where it appears).",
"",
"pub(crate) const A1_NUM: &[f64] = "+e_slice(A1n)+";",
"pub(crate) const A2_NUM: &[f64] = "+e_slice(A2n)+";"]
for name,C in (("C1",C1),("C2",C2),("C1P",C1p)):
    out.append(f"pub(crate) const {name}: [&[f64]; 6] = [")
    for c in C: out.append("    "+e_slice(c)+",")
    out.append("];")
out.append("pub(crate) const A3: [&[f64]; 7] = [")
for i in range(ORD+1): out.append("    "+n_slice(A3,i,ORD-i)+",")
out.append("];")
out.append("pub(crate) const C3: [[&[f64]; 7]; 6] = [")
for c in C3:
    out.append("    [")
    for i in range(ORD+1): out.append("        "+n_slice(c,i,ORD-i)+",")
    out.append("    ],")
out.append("];")
open("coef.rs","w").write("\n".join(out)+"\n")
for l,c in enumerate(C1p): print("C1p",l+1,sorted(c.items()))
print("A3",sorted(A3.items())[:6])
print("C1_1",sorted(C1[0].items()))
