import { describe, expect, it } from 'vitest'
import { displayName, initials } from './user'
import type { NamedUser } from './user'

const make = (over: Partial<NamedUser>): NamedUser => ({
  firstName: null,
  lastName: null,
  email: 'survivor@ashwend.game',
  ...over,
})

describe('displayName', () => {
  it('prefers the first name', () => {
    expect(displayName(make({ firstName: 'Ada' }))).toBe('Ada')
  })

  it('falls back to the email when no first name', () => {
    expect(displayName(make({ firstName: null }))).toBe('survivor@ashwend.game')
  })

  it('treats a blank first name as missing', () => {
    expect(displayName(make({ firstName: '   ' }))).toBe(
      'survivor@ashwend.game',
    )
  })
})

describe('initials', () => {
  it('combines first and last initials', () => {
    expect(initials(make({ firstName: 'Ada', lastName: 'Vex' }))).toBe('AV')
  })

  it('uses two letters of the first name alone', () => {
    expect(initials(make({ firstName: 'Ada' }))).toBe('AD')
  })

  it('derives from the email handle when unnamed', () => {
    expect(initials(make({ email: 'knox@ashwend.game' }))).toBe('KN')
  })
})
