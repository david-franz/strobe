import { p1, p2, p3, p4, p5, sound, note, sine, setbpm } from "strobe";

setbpm(130);

p1 (
  sound("[sn ~ [bd hh] hh*<4 8>]")
)

p2 (
  sound("<cp(5,8) bd(3, 8)>")
)

p3 (
  note(`
    <
      [c3 [eb4 d3] c3 [eb4 d3] eb3 c3 eb3 c3] 
      [c3 ~ c3 [eb4 d3] eb3 c3 eb3 c3] 
      [c4 [eb4 d3] ~ [eb4 d3] eb3 c4 eb3 c4] 
      [c3 ~ c3 [eb4 d3] eb3 [d4 c4] eb3 c3]
    >`
  )
    .sine()
    .lpf(100)
    .gain(0.6)
)

p4 (
  note("[c2 d2 eb2 d2]*8")
    .tri()
    .lpf(1000)
    .gain(0.7)
)

p5 (
  note("c2*8").lpf( sine.range(300,1200).slow(8) ) 
)