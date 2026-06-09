import { describe, it, expect, beforeAll } from 'vitest';
import { ensureBuild, runPetal, showIrJson, userTerms, termsByOp } from './helpers';

beforeAll(() => {
  ensureBuild();
});

describe('method syntax', () => {
  it('calls builtin len() on a list', () => {
    const out = runPetal('print([1,2,3].len())');
    expect(out.trim()).toBe('3');
  });

  it('calls builtin len() on a string', () => {
    const out = runPetal('print("hello".len())');
    expect(out.trim()).toBe('5');
  });

  it('calls map() as a method', () => {
    const out = runPetal('print([1,2,3].map(fn(x) -> x * 2))');
    expect(out.trim()).toBe('[2, 4, 6]');
  });

  it('calls filter() as a method', () => {
    const out = runPetal('print([1,2,3].filter(fn(x) -> x > 1))');
    expect(out.trim()).toBe('[2, 3]');
  });

  it('calls a callable record field', () => {
    const out = runPetal('let r = {greet: fn(x) -> x}\nprint(r.greet("hi"))');
    expect(out.trim()).toBe('hi');
  });

  it('calls push() as a method', () => {
    const out = runPetal('let items = [1,2,3]\nitems.push(4)\nprint(items)');
    expect(out.trim()).toBe('[1, 2, 3, 4]');
  });

  it('chains method calls', () => {
    const out = runPetal('print([1,2,3].map(fn(x) -> x * 2).filter(fn(x) -> x > 2))');
    expect(out.trim()).toBe('[4, 6]');
  });

  it('combines method syntax with pipe operator', () => {
    const out = runPetal('[1,2,3].map(fn(x) -> x * 2) |> print');
    expect(out.trim()).toBe('[2, 4, 6]');
  });

  it('emits MethodCall in IR', () => {
    const ir = showIrJson('[1,2,3].len()');
    const methodCalls = termsByOp(ir, 'MethodCall');
    expect(methodCalls.length).toBe(1);
  });
});
