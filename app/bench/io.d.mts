/** N9 bench Node IO 薄层声明(io.mjs 的类型面;见 io.mjs 头注释)。 */

export declare function readFixture(n: number): {
  session: Record<string, unknown>;
  messages: Array<Record<string, unknown>>;
};

export declare function writeReport(report: Record<string, unknown>): string;
