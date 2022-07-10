encrypt = function (a) {
    var b = a.split(""), c = [1798681818, b, function (d, e, f) {
            var k = f.length;
            d.forEach(function (l, m, n) {
                this.push(n[m] = f[(f.indexOf(l) - f.indexOf(this[m]) + m + k--) % f.length])
            }, e.split(""))
        }
            , function () {
                for (var d = 64, e = []; ++d - e.length - 32;)
                    switch (d) {
                        case 58:
                            d = 96;
                            continue;
                        case 91:
                            d = 44;
                            break;
                        case 65:
                            d = 47;
                            continue;
                        case 46:
                            d = 153;
                        case 123:
                            d -= 58;
                        default:
                            e.push(String.fromCharCode(d))
                    }
                return e
            }
            , 1415564298, function (d, e) {
                e = (e % d.length + d.length) % d.length;
                d.splice(-e).reverse().forEach(function (f) {
                    d.unshift(f)
                })
            }
            , 715387316, 1381228121, -1079328693, -2083707900, -115505167, 1619563971, 1937928146, 2075486511, 144959113, -1688673264, -730619303, -1961951785, function (d, e) {
                for (e = (e % d.length + d.length) % d.length; e--;)
                    d.unshift(d.pop())
            }
            , 1570199588, 102266298, null, function (d, e) {
                e = (e % d.length + d.length) % d.length;
                var f = d[0];
                d[0] = d[e];
                d[e] = f
            }
            , -584962662, function (d) {
                d.reverse()
            }
            , 1367128684, -1702238480, function (d, e) {
                e = (e % d.length + d.length) % d.length;
                d.splice(e, 1)
            }
            , 969960864, 2137087855, -1326287897, 841245699, 2137087855, 1894596299, b, 1894596299, 1694613204, function (d) {
                for (var e = d.length; e;)
                    d.push(d.splice(--e, 1)[0])
            }
            , 591013114, -271649020, function (d, e) {
                e = (e % d.length + d.length) % d.length;
                d.splice(0, 1, d.splice(e, 1, d[0])[0])
            }
            , 1389546098, -1395447140, -1848258562, b, 1389546098, -1433894087, null, -1359560084, "push", 263480107, null, -584962662, -206410820, function (d, e) {
                d.push(e)
            }
        ];
    c[21] = c;
    c[47] = c;
    c[51] = c;
    try {
        c[27](c[1], c[9]),
            c[22](c[34], c[50]),
            c[22](c[44], c[4]),
            c[18](c[47], c[15]),
            c[4](c[30], c[3]),
            c[46](c[33], c[24]),
            c[25](c[36], c[16]),
            c[9](c[41]),
            c[7](c[32], c[50]),
            c[0](c[29], c[17]),
            c[45](c[29], c[53]),
            c[39](c[6], c[10]),
            c[45](c[6], c[46]),
            c[40](c[13], c[0]),
            c[50](c[1]),
            c[28](c[34], c[38]),
            c[52](c[43], c[32]),
            c[49](c[13]),
            c[9](c[16], c[51]),
            c[47](c[0], c[45]),
            c[9](c[20], c[44]),
            c[47](c[25], c[53]),
            c[40](c[43], c[8]),
            c[9](c[46], c[15]),
            c[47](c[43], c[4]),
            c[52](c[0], c[55]),
            c[29](c[46], c[21]),
            c[19](c[4], c[51]),
            c[50](c[4], c[46]),
            c[25](c[46], c[13]),
            c[0](c[47], c[21]),
            c[4](c[33], c[10]),
            c[46](c[7], c[38], c[47]()),
            c[16](c[33], c[2]),
            c[29](c[51], c[34]),
            c[13](c[51]),
            c[6](c[22], c[0]),
            c[51](c[10], c[11]),
            c[52](c[45]),
            c[5](c[45], c[23]),
            c[36](c[20], c[34])
    } catch (d) {
        return "enhanced_except_1JUBq-r-_w8_" + a
    }
    return b.join("")
}


if (typeof require !== 'undefined' && require.main === module) {
    const myArgs = process.argv.slice(2);
    const result = encrypt(myArgs[0])
    console.log("youtube" + "zero_result:" + result)
}


